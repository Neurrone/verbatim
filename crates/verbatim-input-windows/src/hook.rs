//! The code that runs on the hook thread: the hook procedure, the decision
//! of each key, the holding of caret keys, the inputs injected for the
//! user, and the thread's one message loop
//! (`docs/design/ownership-input.md`, "The ownership check on the hook").
//!
//! Nothing here blocks or takes a lock: `cargo xtask ci` checks that this
//! module names nothing that does (`xtask/src/ownership.rs`, the hook
//! rule). Everything a key or an injected input causes leaves through the
//! one report callback, in the order the thread handled it, and the
//! thread numbers every user input ([`KeySeq`]) in that same order.

use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_NUMLOCK};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetForegroundWindow, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED,
    LLKHF_INJECTED, MSG, PM_REMOVE, PeekMessageW, WM_KEYDOWN, WM_SYSKEYDOWN,
};

use verbatim_input::state::{Decision, DecisionMachine, EmittedGesture, KeySpeechEffect};
use verbatim_input::{KeyDecision, KeyEvent};
use verbatim_model::{KeySeq, TraceId};

use crate::{
    Effects, ForegroundView, HeldStroke, Injected, KeyHolding, KeyReport, KeyReportFn,
    OWN_INPUT_TAG, SpeechEffectFn, WM_INJECTED, may_hold, passed_on_sequence, typed,
};

/// Per-hook-thread state reached by the hook procedure.
///
/// The hook procedure is a bare `extern "system"` function with no user
/// pointer, so it reaches its state through this thread-local. The procedure
/// only ever runs on the thread that installed the hook, so a thread-local is
/// exactly the right scope and needs no synchronization.
pub(crate) struct HookState {
    machine: DecisionMachine,
    speech: SpeechEffectFn,
    reports: KeyReportFn,
    typing: typed::Typing,
    holding: Arc<KeyHolding>,
    /// The keys whose press was held back, whose release is held back too,
    /// so the application never sees a release without its press.
    held_downs: HashSet<(u16, bool)>,
    /// The key transitions held back or deferred and not yet seen passed
    /// on, in the order pressed; while any is, every later transition the
    /// hook would pass is deferred behind them.
    outstanding: VecDeque<Outstanding>,
    /// The last place in line given out ([`HeldStroke::sequence`]).
    sequence: u32,
    /// The last key number given out, to a key press or an injected
    /// gesture: this thread is the one minter of key numbers.
    last_key: KeySeq,
    /// Core's view of the foreground window ([`ForegroundView`]).
    foreground: Arc<ForegroundView>,
    /// The token an injected input's message carries ([`WM_INJECTED`]).
    token: usize,
    /// Reads the window in front: `GetForegroundWindow`, a local read, or
    /// a fixed window in tests.
    front: fn() -> HWND,
}

/// A key transition held back or deferred, not yet seen passed on.
#[derive(Clone, Copy, Debug)]
struct Outstanding {
    sequence: u32,
    /// A deferred transition's key and whether it is a press, which the
    /// translation of typed text takes as already made; `None` for a held
    /// caret key, which is no modifier.
    transition: Option<(u16, bool)>,
}

impl HookState {
    /// A hook state with nothing held, deciding with `machine`.
    pub(crate) fn new(
        machine: DecisionMachine,
        (speech, reports, holding, foreground): Effects,
        typing: typed::Typing,
        token: usize,
    ) -> Self {
        Self {
            machine,
            speech,
            reports,
            typing,
            holding,
            held_downs: HashSet::new(),
            outstanding: VecDeque::new(),
            sequence: 0,
            last_key: KeySeq::NONE_YET,
            foreground,
            token,
            front: || {
                // SAFETY: a local read with no preconditions.
                unsafe { GetForegroundWindow() }
            },
        }
    }

    /// Gives out the next key number.
    fn next_key(&mut self) -> KeySeq {
        self.last_key = KeySeq::after(self.last_key);
        self.last_key
    }

    /// Notes a transition held back or deferred, giving it the next place
    /// in line.
    fn queue(&mut self, transition: Option<(u16, bool)>) -> u32 {
        self.sequence = self.sequence.checked_add(1).unwrap_or(1);
        self.outstanding.push_back(Outstanding {
            sequence: self.sequence,
            transition,
        });
        self.sequence
    }

    /// Notes that the key passed on with place `sequence` reached the hook
    /// on its way to the application, and with it every key ahead of it,
    /// since keys are passed on in order: one Windows dropped is not waited
    /// for once a later one arrives.
    fn passed_on(&mut self, sequence: u32) {
        if !self
            .outstanding
            .iter()
            .any(|outstanding| outstanding.sequence == sequence)
        {
            return;
        }
        while let Some(front) = self.outstanding.pop_front() {
            if front.sequence == sequence {
                break;
            }
        }
    }

    /// The deferred transitions not yet passed on, in order.
    fn deferred_transitions(&self) -> Vec<(u16, bool)> {
        self.outstanding
            .iter()
            .filter_map(|outstanding| outstanding.transition)
            .collect()
    }
}

thread_local! {
    pub(crate) static HOOK_STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
}

/// Microseconds since the Unix epoch on the run's shared clock, as
/// outposts stamp their observations ([`verbatim_process::clock`]).
fn unix_us() -> u64 {
    verbatim_process::clock::now_us()
}

/// Runs the hook thread's message loop until `WM_QUIT` (or a `GetMessageW`
/// error) ends it, handling each input injected for the user
/// ([`Injected`]) as it comes. A low-level hook only fires while its
/// installing thread pumps messages, and Windows calls the hook procedure
/// from within this loop's waits, so keys and injected inputs are handled
/// one at a time, in the one order the thread takes them. The thread runs
/// no other loop while the hook is installed.
pub(crate) fn pump_messages() {
    let token = thread_token();
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
        if let Some(injected) = injected_in(&msg, token) {
            HOOK_STATE.with(|state| {
                // Never already borrowed: this runs from the loop itself,
                // between the hook procedure's calls, which borrow the
                // state only for their own length.
                if let Ok(mut state) = state.try_borrow_mut()
                    && let Some(state) = state.as_mut()
                {
                    inject(state, injected);
                }
            });
        }
    }
}

/// The input `msg` carries, when it is one an [`crate::Injector`] posted:
/// a thread message (no window) of [`WM_INJECTED`] carrying `token`. Any
/// other message, another process's `WM_APP` or one to a COM window on
/// this thread, is left alone, its `LPARAM` never read.
fn injected_in(msg: &MSG, token: usize) -> Option<Injected> {
    if msg.message != WM_INJECTED || !msg.hwnd.is_invalid() || msg.wParam.0 != token {
        return None;
    }
    // SAFETY: a thread message of `WM_INJECTED` carrying the token was
    // posted by `Injector::inject`, whose `LPARAM` is a box it gave up,
    // taken off the queue this once.
    Some(*unsafe { Box::from_raw(msg.lParam.0 as *mut Injected) })
}

/// The token this thread's injected inputs carry, kept in its state.
fn thread_token() -> usize {
    HOOK_STATE.with(|state| state.borrow().as_ref().map_or(0, |state| state.token))
}

/// Takes the injected inputs still queued once the hook is removed and
/// drops them: nothing is left to act on them. One posted after this
/// drain, before the thread has exited, is never taken, and its box leaks
/// at exit; the idle waiter that posted it times out.
pub(crate) fn drop_injected() {
    let token = thread_token();
    let mut msg = MSG::default();
    // SAFETY: `msg` is a valid, owned message buffer; only this thread's
    // `WM_APP` messages are removed, and only the injector's boxes taken.
    while unsafe { PeekMessageW(&raw mut msg, None, WM_INJECTED, WM_INJECTED, PM_REMOVE) }.as_bool()
    {
        drop(injected_in(&msg, token));
    }
}

/// Handles one input injected for the user: a gesture is numbered as a key
/// press is, cancels speech through its number as a key does, and is
/// reported with it; a barrier is reported behind everything handled
/// before it.
pub(crate) fn inject(state: &mut HookState, injected: Injected) {
    match injected {
        Injected::Gesture(gesture) => {
            let key_seq = state.next_key();
            (state.speech)(KeySpeechEffect::Cancel, key_seq);
            (state.reports)(KeyReport::Gesture {
                gesture: EmittedGesture {
                    trace_id: TraceId::mint(),
                    gesture,
                    // An injected gesture is always a single, first press;
                    // multi-press counting applies to real key streams.
                    repeat: 0,
                },
                key_seq,
                pressed_at_us: unix_us(),
            });
        }
        Injected::Barrier(token) => (state.reports)(KeyReport::Barrier(token)),
    }
}

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

/// The `WH_KEYBOARD_LL` procedure. Runs on the hook thread for every key
/// transition system-wide; see the crate's never-block constraint.
///
/// # Safety
///
/// Called by the operating system with `WH_KEYBOARD_LL` conventions: when
/// `code == HC_ACTION`, `lparam` points to a valid [`KBDLLHOOKSTRUCT`]. The
/// function only dereferences it in that case.
pub(crate) unsafe extern "system" fn keyboard_hook(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
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

/// What [`hold`] did with a key transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hold {
    /// Left as decided.
    No,
    /// A caret key's press held back for its caret to be read, with its
    /// place in line.
    Held(u32),
    /// A transition that would have passed, deferred behind a held key,
    /// with its place in line.
    Deferred(u32),
}

/// Holds a caret key back while holding is on, and its release with it,
/// swallowing both in `decision`; the reducer has the key sent again once
/// the caret has been read. While any key held back has not yet reached
/// the application, every later transition the hook would pass is
/// deferred, swallowed and sent again behind it, so that no key overtakes
/// a held one (`docs/design/key-holding.md`, "Keys behind a held key").
/// Nothing is held or deferred unless Core's view of the foreground is
/// about the window in front and lets keys be held there, which it does
/// not while the window's process has higher integrity than Verbatim's,
/// since Windows would drop a key sent again to it ([`ForegroundView`]);
/// Verbatim's own keys are never held, and the
/// lock keys are never deferred: Windows translates the numpad's keys by
/// Num Lock's state before the hook sees them, and a lock key's new state
/// is announced from Windows' own, so a lock key held back would turn a
/// numpad key behind it into the wrong key and be announced with its old
/// state.
fn hold(state: &mut HookState, event: KeyEvent, own: bool, decision: &mut Decision) -> Hold {
    let stroke = (event.vk, event.extended);
    if !event.pressed && state.held_downs.remove(&stroke) {
        // The held press is sent again with its release.
        decision.decision = KeyDecision::Swallow;
        return Hold::No;
    }
    if own || verbatim_input::ToggleKey::from_vk(event.vk).is_some() {
        return Hold::No;
    }
    let held = event.pressed
        && decision
            .observed
            .as_ref()
            .is_some_and(|observed| state.holding.holds(&observed.gesture));
    let deferred = !held && decision.decision == KeyDecision::Pass && !state.outstanding.is_empty();
    // Read only for a key that would be held or deferred.
    if (held || deferred)
        && !may_hold(
            typed::window_handle((state.front)()),
            state.foreground.get(),
        )
    {
        return Hold::No;
    }
    if held {
        state.held_downs.insert(stroke);
        decision.decision = KeyDecision::Swallow;
        return Hold::Held(state.queue(None));
    }
    if deferred {
        decision.decision = KeyDecision::Swallow;
        return Hold::Deferred(state.queue(Some((event.vk, event.pressed))));
    }
    Hold::No
}

/// Reports what a key press that reaches the application, now or once
/// deferred, did: the text it types, or, for a key that types nothing and
/// completed no observed gesture, that it may have edited the text.
fn report_passed(
    state: &mut HookState,
    event: KeyEvent,
    scan_code: u32,
    key_seq: KeySeq,
    was_observed: bool,
) {
    let pressed_at_us = unix_us();
    let trace_id = TraceId::mint();
    let deferred = state.deferred_transitions();
    let foreground = (state.front, state.foreground.get());
    match state
        .typing
        .translate((event.vk, scan_code), &deferred, foreground)
    {
        Some(text) => (state.reports)(KeyReport::Typed {
            trace_id,
            key_seq,
            text,
            pressed_at_us,
        }),
        None if !was_observed && may_edit(event.vk) => (state.reports)(KeyReport::Passed {
            trace_id,
            key_seq,
            pressed_at_us,
        }),
        None => {}
    }
}

/// Decides one key transition with the hook thread's state, carries out its
/// effect on speech, and reports what it caused, each report carrying the
/// key's number; `scan_code` and `extra_info` are the key event's own.
pub(crate) fn decide(
    state: &mut HookState,
    event: KeyEvent,
    scan_code: u32,
    extra_info: usize,
) -> KeyDecision {
    // A key Verbatim passes on, with the modifiers sent around it, goes
    // straight to the application: it was decided and reported when it
    // was pressed.
    if let Some(sequence) = passed_on_sequence(extra_info) {
        if sequence != 0 {
            state.passed_on(sequence);
        }
        return KeyDecision::Pass;
    }
    // Num Lock decides what the numpad's operator keys are: the
    // keyboard's own state, read locally before each key.
    // SAFETY: GetKeyState takes any virtual-key code.
    let num_lock = unsafe { GetKeyState(i32::from(VK_NUMLOCK.0)) } & 1 != 0;
    state.machine.set_num_lock(num_lock);
    let mut decision = state.machine.on_key(event, Instant::now());
    let own = extra_info == OWN_INPUT_TAG;
    // Whether the key reaches the application, now or once deferred.
    let reaches_application = decision.decision == KeyDecision::Pass;
    let hold = hold(state, event, own, &mut decision);
    let stroke = |sequence| HeldStroke {
        vk: event.vk,
        scan_code,
        extended: event.extended,
        sequence,
    };
    let key_seq = if event.pressed {
        state.next_key()
    } else {
        state.last_key
    };
    if let Some(effect) = decision.speech
        && !own
    {
        (state.speech)(effect, key_seq);
    }
    if let Some(emitted) = decision.emitted {
        (state.reports)(KeyReport::Gesture {
            gesture: emitted,
            key_seq,
            pressed_at_us: unix_us(),
        });
    }
    let was_observed = decision.observed.is_some();
    if let Some(observed) = decision.observed
        && !own
    {
        let pressed_at_us = unix_us();
        (state.reports)(if let Hold::Held(sequence) = hold {
            KeyReport::Held {
                gesture: observed,
                key_seq,
                pressed_at_us,
                stroke: stroke(sequence),
            }
        } else {
            KeyReport::Observed {
                gesture: observed,
                key_seq,
                pressed_at_us,
            }
        });
    }
    if event.pressed
        && reaches_application
        && !matches!(hold, Hold::Held(_))
        && !decision.shared_modifier
        && !own
    {
        report_passed(state, event, scan_code, key_seq, was_observed);
    }
    // After what the key did, so that whoever passes it on has been told
    // of it first.
    if let Hold::Deferred(sequence) = hold {
        (state.reports)(KeyReport::Deferred {
            stroke: stroke(sequence),
            pressed: event.pressed,
        });
    }
    // A lock key reaching the operating system is reported, for its
    // new state to be announced, as NVDA announces it. The Verbatim
    // modifier passed in share mode is not: the screen reader behind
    // Verbatim decides whether it reaches the operating system, and
    // announces the state itself if it lets it through.
    if event.pressed
        && reaches_application
        && !decision.shared_modifier
        && let Some(toggle) = verbatim_input::ToggleKey::from_vk(event.vk)
    {
        (state.reports)(KeyReport::Gesture {
            gesture: EmittedGesture {
                trace_id: TraceId::mint(),
                gesture: toggle.gesture(),
                repeat: 0,
            },
            key_seq,
            pressed_at_us: unix_us(),
        });
    }
    // A harness's stroke is complete with its last key event: say
    // so behind everything the stroke caused.
    if let Some(numbered) = u64::try_from(extra_info)
        .ok()
        .and_then(verbatim_input::harness::decode)
        && numbered.last
    {
        (state.reports)(KeyReport::Handled(numbered.number));
    }
    decision.decision
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use verbatim_input::GestureMap;
    use verbatim_input::state::DecisionConfig;
    use verbatim_model::{GestureId, HookForeground, WindowHandle};

    use super::*;
    use crate::{Injector, PASSED_ON_TAG, passed_on_info};

    /// A burst of strokes, each Verbatim+V with a harness number, decided
    /// while nothing takes the reports: `decide` reports every gesture and
    /// every stroke's end, in order, with none lost.
    #[test]
    fn a_burst_larger_than_the_old_bound_is_delivered_in_order_with_none_lost() {
        const CAPS: u16 = 0x14;
        const V: u16 = 0x56;
        const STROKES: u64 = 1_000;
        let key_seq = |vk, pressed| KeyEvent {
            vk,
            scan_code: 0,
            extended: false,
            injected: true,
            pressed,
        };
        let map = GestureMap::new([GestureId::parse("kb:v+verbatim").expect("valid id")]);
        let reports = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&reports);
        let mut state = HookState::new(
            DecisionMachine::new(DecisionConfig::default(), map.into_shared()),
            (
                Box::new(|_, _| {}),
                Box::new(move |report| sink.lock().unwrap().push(report)),
                Arc::new(KeyHolding::new([])),
                Arc::new(ForegroundView::new()),
            ),
            typed::Typing::default(),
            0,
        );
        assert_eq!(
            decide(&mut state, key_seq(CAPS, true), 0, 0),
            KeyDecision::Swallow
        );
        for number in 1..=STROKES {
            let last = usize::try_from(verbatim_input::harness::encode(number, true)).unwrap();
            assert_eq!(
                decide(&mut state, key_seq(V, true), 0, 0),
                KeyDecision::Swallow
            );
            assert_eq!(
                decide(&mut state, key_seq(V, false), 0, last),
                KeyDecision::Swallow
            );
        }
        let expected: Vec<String> = (1..=STROKES)
            .flat_map(|number| {
                [
                    "gesture kb:v+verbatim".to_owned(),
                    format!("handled {number}"),
                ]
            })
            .collect();
        assert_eq!(described(&reports.lock().unwrap()), expected);
    }

    /// A hook state observing Right Arrow and Escape, holding Right Arrow,
    /// whose reports are collected in `reports`.
    fn holding_state(reports: &Arc<Mutex<Vec<KeyReport>>>) -> (HookState, Arc<KeyHolding>) {
        let right = GestureId::parse("kb:rightarrow").expect("valid id");
        let escape = GestureId::parse("kb:escape").expect("valid id");
        let map = GestureMap::new([]).with_observed([right.clone(), escape]);
        let holding = Arc::new(KeyHolding::new([right]));
        let sink = Arc::clone(reports);
        let foreground = Arc::new(ForegroundView::new());
        foreground.set(Some(IN_FRONT));
        let mut state = HookState::new(
            DecisionMachine::new(DecisionConfig::default(), map.into_shared()),
            (
                Box::new(|_, _| {}),
                Box::new(move |report| sink.lock().unwrap().push(report)),
                Arc::clone(&holding),
                foreground,
            ),
            typed::Typing::default(),
            0,
        );
        state.front = window_in_front;
        (state, holding)
    }

    /// The window the hook's tests have in front: not a real window, so
    /// that nothing depends on the desktop the tests run on; its keys are
    /// translated with the test thread's own keyboard layout.
    fn window_in_front() -> HWND {
        HWND(7 as *mut core::ffi::c_void)
    }

    /// Core's view of [`window_in_front`], whose keys may be held.
    const IN_FRONT: HookForeground = HookForeground {
        window: WindowHandle(7),
        holds_keys: true,
        keyboard_thread: None,
    };

    /// The letter X, a key neither held nor observed, pressed or released.
    fn letter_x(pressed: bool) -> KeyEvent {
        KeyEvent {
            vk: 0x58,
            scan_code: 0x2D,
            extended: false,
            injected: false,
            pressed,
        }
    }

    /// What each report says, for comparing a sequence of them.
    fn described(reports: &[KeyReport]) -> Vec<String> {
        reports
            .iter()
            .map(|report| match report {
                KeyReport::Held {
                    gesture, stroke, ..
                } => format!("held {} #{}", gesture.gesture, stroke.sequence),
                KeyReport::Deferred { stroke, pressed } => format!(
                    "deferred {:#04x} {} #{}",
                    stroke.vk,
                    if *pressed { "down" } else { "up" },
                    stroke.sequence
                ),
                KeyReport::Observed { gesture, .. } => format!("observed {}", gesture.gesture),
                KeyReport::Passed { .. } => "passed".to_owned(),
                KeyReport::Typed { text, .. } => format!("typed {text}"),
                KeyReport::Gesture { gesture, .. } => format!("gesture {}", gesture.gesture),
                KeyReport::Handled(number) => format!("handled {number}"),
                KeyReport::Barrier(token) => format!("barrier {token}"),
            })
            .collect()
    }

    /// While a held key has not reached the application, every later key
    /// the hook would pass, a typed letter and its release included, is
    /// deferred behind it in the order pressed, and the letter is still
    /// reported typed; a later caret key is held in its own turn. Once the
    /// keys passed on come back through the hook, up to the last one held
    /// back, keys pass again; one passed on with a later place clears
    /// those before it, which Windows may have dropped.
    #[test]
    fn keys_behind_a_held_key_are_deferred_in_order_until_it_is_passed_on() {
        let reports = Arc::new(Mutex::new(Vec::new()));
        let (mut state, holding) = holding_state(&reports);
        holding.set(true);
        let swallowed = KeyDecision::Swallow;
        let pass = KeyDecision::Pass;
        assert_eq!(decide(&mut state, right_arrow(true), 0x4D, 0), swallowed);
        assert_eq!(decide(&mut state, right_arrow(false), 0x4D, 0), swallowed);
        assert_eq!(decide(&mut state, letter_x(true), 0x2D, 0), swallowed);
        assert_eq!(decide(&mut state, right_arrow(true), 0x4D, 0), swallowed);
        assert_eq!(decide(&mut state, letter_x(false), 0x2D, 0), swallowed);
        assert_eq!(
            described(&reports.lock().unwrap()),
            [
                "held kb:rightarrow #1",
                "typed x",
                "deferred 0x58 down #2",
                "held kb:rightarrow #3",
                "deferred 0x58 up #4",
            ]
        );
        // The first Right Arrow and the letter come back passed on: the
        // keys behind them still wait.
        assert_eq!(
            decide(&mut state, right_arrow(true), 0x4D, passed_on_info(1)),
            pass
        );
        assert_eq!(
            decide(&mut state, letter_x(true), 0x2D, passed_on_info(2)),
            pass
        );
        reports.lock().unwrap().clear();
        assert_eq!(decide(&mut state, letter_x(true), 0x2D, 0), swallowed);
        assert_eq!(
            described(&reports.lock().unwrap()),
            ["typed x", "deferred 0x58 down #5"]
        );
        // The last key held back comes back, clearing the second Right
        // Arrow and the letter's release though they never came: a key
        // pressed now passes.
        assert_eq!(
            decide(&mut state, letter_x(true), 0x2D, passed_on_info(5)),
            pass
        );
        assert_eq!(decide(&mut state, letter_x(false), 0x2D, 0), pass);
        // The modifiers and releases sent around a held key carry no
        // place, and clear nothing.
        assert_eq!(decide(&mut state, right_arrow(true), 0x4D, 0), swallowed);
        assert_eq!(
            decide(&mut state, right_arrow(false), 0x4D, passed_on_info(0)),
            pass
        );
        assert_eq!(decide(&mut state, letter_x(true), 0x2D, 0), swallowed);
    }

    /// A lock key pressed while a held key has not reached the application
    /// passes at once, press and release, and is not deferred: its state is
    /// announced from Windows' own, and Windows translates the numpad's
    /// keys by Num Lock's state before the hook sees them. Caps Lock, the
    /// Verbatim key by default, reaches the application on a double press,
    /// whose first press is the Verbatim key's and swallowed. A letter after
    /// them is still deferred.
    #[test]
    fn a_lock_key_passes_at_once_behind_a_held_key() {
        let reports = Arc::new(Mutex::new(Vec::new()));
        let (mut state, holding) = holding_state(&reports);
        holding.set(true);
        assert_eq!(
            decide(&mut state, right_arrow(true), 0x4D, 0),
            KeyDecision::Swallow
        );
        let lock = |vk, scan_code, extended, pressed| KeyEvent {
            vk,
            scan_code,
            extended,
            injected: false,
            pressed,
        };
        let mut decided = Vec::new();
        for (vk, scan_code, extended, presses) in [
            (0x14, 0x3A, false, 2),
            (0x90, 0x45, true, 1),
            (0x91, 0x46, false, 1),
        ] {
            for _ in 0..presses {
                for pressed in [true, false] {
                    let event = lock(vk, scan_code, extended, pressed);
                    decided.push((vk, pressed, decide(&mut state, event, scan_code, 0)));
                }
            }
        }
        let (swallowed, pass) = (KeyDecision::Swallow, KeyDecision::Pass);
        assert_eq!(
            decided,
            [
                (0x14, true, swallowed),
                (0x14, false, swallowed),
                (0x14, true, pass),
                (0x14, false, pass),
                (0x90, true, pass),
                (0x90, false, pass),
                (0x91, true, pass),
                (0x91, false, pass),
            ]
        );
        assert_eq!(
            decide(&mut state, letter_x(true), 0x2D, 0),
            KeyDecision::Swallow
        );
        assert_eq!(
            described(&reports.lock().unwrap()),
            [
                "held kb:rightarrow #1",
                "gesture kb:capslock",
                "gesture kb:numlock",
                "gesture kb:scrolllock",
                "typed x",
                "deferred 0x58 down #2"
            ]
        );
    }

    /// While Core's view of the window in front says keys may not be held
    /// for it, as for a window of higher integrity than Verbatim's, a caret
    /// key passes unheld and is reported observed, and no key is deferred
    /// behind a key held before the window came to the front, whose
    /// release is still held back with it.
    #[test]
    fn no_key_is_held_where_core_says_none_may_be() {
        let reports = Arc::new(Mutex::new(Vec::new()));
        let (mut state, holding) = holding_state(&reports);
        holding.set(true);
        assert_eq!(
            decide(&mut state, right_arrow(true), 0x4D, 0),
            KeyDecision::Swallow
        );
        state.foreground.set(Some(HookForeground {
            holds_keys: false,
            ..IN_FRONT
        }));
        assert_eq!(
            decide(&mut state, right_arrow(false), 0x4D, 0),
            KeyDecision::Swallow
        );
        assert_eq!(
            decide(&mut state, letter_x(true), 0x2D, 0),
            KeyDecision::Pass
        );
        assert_eq!(
            decide(&mut state, right_arrow(true), 0x4D, 0),
            KeyDecision::Pass
        );
        assert_eq!(
            described(&reports.lock().unwrap()),
            ["held kb:rightarrow #1", "typed x", "observed kb:rightarrow"]
        );
        // Back at a window where keys may be held, they are held again.
        state.foreground.set(Some(IN_FRONT));
        assert_eq!(
            decide(&mut state, right_arrow(true), 0x4D, 0),
            KeyDecision::Swallow
        );
    }

    /// Core's view is applied only to the window it was decided for: while
    /// another window is in front, before Core has caught up with it, or
    /// before Core has any view, no key is held.
    #[test]
    fn no_key_is_held_until_core_has_a_view_of_the_window_in_front() {
        let reports = Arc::new(Mutex::new(Vec::new()));
        let (mut state, holding) = holding_state(&reports);
        holding.set(true);
        state.foreground.set(Some(HookForeground {
            window: WindowHandle(8),
            ..IN_FRONT
        }));
        assert_eq!(
            decide(&mut state, right_arrow(true), 0x4D, 0),
            KeyDecision::Pass
        );
        state.foreground.set(None);
        assert_eq!(
            decide(&mut state, right_arrow(true), 0x4D, 0),
            KeyDecision::Pass
        );
        assert_eq!(
            described(&reports.lock().unwrap()),
            ["observed kb:rightarrow", "observed kb:rightarrow"]
        );
        state.foreground.set(Some(IN_FRONT));
        assert_eq!(
            decide(&mut state, right_arrow(true), 0x4D, 0),
            KeyDecision::Swallow
        );
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
        let (mut state, holding) = holding_state(&reports);
        holding.set(true);
        assert_eq!(
            decide(&mut state, right_arrow(true), 0x4D, 0),
            KeyDecision::Swallow
        );
        assert_eq!(
            decide(&mut state, right_arrow(false), 0x4D, 0),
            KeyDecision::Swallow
        );
        // Passed on, it reaches the hook again and goes straight through.
        assert_eq!(
            decide(&mut state, right_arrow(true), 0x4D, passed_on_info(1)),
            KeyDecision::Pass
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
        let KeyReport::Held { stroke, .. } = reports[0] else {
            panic!("expected Right Arrow held first: {reports:?}");
        };
        assert_eq!(
            stroke,
            HeldStroke {
                vk: 0x27,
                scan_code: 0x4D,
                extended: true,
                sequence: 1
            }
        );
        assert_eq!(
            described(&reports),
            [
                "held kb:rightarrow #1",
                "observed kb:escape",
                "observed kb:rightarrow",
            ]
        );
    }

    /// A key carrying [`PASSED_ON_TAG`], a held key Verbatim passes on,
    /// passes straight through, reported to nobody, though holding is on;
    /// a key the end-to-end harness injects, with its stroke number, is the
    /// user's and is held, and its stroke's end reported behind it.
    #[test]
    fn a_key_passed_on_passes_untouched_and_a_harness_key_is_held() {
        let reports = Arc::new(Mutex::new(Vec::new()));
        let (mut state, holding) = holding_state(&reports);
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
        assert_eq!(
            described(&reports.lock().unwrap()),
            ["held kb:rightarrow #1", "handled 7"]
        );
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

    /// A hook state binding nothing, whose speech effects and reports are
    /// written in `log` in the order made, each with its key number.
    fn logging_state(log: &Arc<Mutex<Vec<String>>>) -> HookState {
        let (speech, reports) = (Arc::clone(log), Arc::clone(log));
        HookState::new(
            DecisionMachine::new(DecisionConfig::default(), GestureMap::new([]).into_shared()),
            (
                Box::new(move |effect, key_seq| {
                    speech.lock().unwrap().push(format!("{effect:?} {key_seq}"));
                }),
                Box::new(move |report| {
                    let key_seq = match &report {
                        KeyReport::Typed { key_seq, .. } | KeyReport::Gesture { key_seq, .. } => {
                            key_seq.to_string()
                        }
                        _ => String::new(),
                    };
                    let described = described(std::slice::from_ref(&report));
                    reports
                        .lock()
                        .unwrap()
                        .push(format!("{} {key_seq}", described[0]));
                }),
                Arc::new(KeyHolding::new([])),
                Arc::new(ForegroundView::new()),
            ),
            typed::Typing::default(),
            0,
        )
    }

    /// A key press, an injected gesture and a second key press are numbered
    /// 1, 2 and 3 in the order handled: the gesture cancels speech through
    /// its own number before it is reported, as a key does, so the key
    /// after it cancels through a higher one and fences off the gesture's
    /// speech, and the reports leave in that order.
    #[test]
    fn an_injected_gesture_is_numbered_between_the_keys_around_it_and_fences_speech_as_a_key_does()
    {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut state = logging_state(&log);
        assert_eq!(
            decide(&mut state, letter_x(true), 0x2D, 0),
            KeyDecision::Pass
        );
        inject(
            &mut state,
            Injected::Gesture(GestureId::parse("kb:verbatim+numpad8").expect("valid id")),
        );
        assert_eq!(
            decide(&mut state, letter_x(true), 0x2D, 0),
            KeyDecision::Pass
        );
        assert_eq!(
            *log.lock().unwrap(),
            [
                "Cancel 1",
                "typed x 1",
                "Cancel 2",
                "gesture kb:numpad8+verbatim 2",
                "Cancel 3",
                "typed x 3",
            ]
        );
    }

    /// An injected barrier is reported behind the reports of every key
    /// decided before it.
    #[test]
    fn an_injected_barrier_is_reported_behind_the_keys_before_it() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut state = logging_state(&log);
        assert_eq!(
            decide(&mut state, letter_x(true), 0x2D, 0),
            KeyDecision::Pass
        );
        inject(&mut state, Injected::Barrier(4));
        assert_eq!(
            *log.lock().unwrap(),
            ["Cancel 1", "typed x 1", "barrier 4 "]
        );
    }

    /// A message the hook did not post itself is never taken for an
    /// injected input: a `WM_APP` with another `WPARAM`, or to a window,
    /// is ignored with its `LPARAM` unread, whatever it holds.
    #[test]
    fn a_message_without_the_token_is_not_taken_for_an_injection() {
        let token = 0x5eed_u16 as usize;
        let foreign = MSG {
            message: WM_INJECTED,
            wParam: WPARAM(0),
            lParam: LPARAM(0x1234),
            ..MSG::default()
        };
        assert_eq!(injected_in(&foreign, token), None);
        let to_a_window = MSG {
            hwnd: HWND(7 as *mut core::ffi::c_void),
            wParam: WPARAM(token),
            ..foreign
        };
        assert_eq!(injected_in(&to_a_window, token), None);
        let item = Box::into_raw(Box::new(Injected::Barrier(3)));
        let posted = MSG {
            wParam: WPARAM(token),
            lParam: LPARAM(item as isize),
            ..foreign
        };
        assert_eq!(injected_in(&posted, token), Some(Injected::Barrier(3)));
    }

    /// An injection posted to thread id 0, which no thread has, fails and
    /// comes back as the error, so it can never run later.
    #[test]
    fn an_injection_that_cannot_be_posted_comes_back() {
        assert_eq!(
            Injector::to_thread(0).inject(Injected::Barrier(9)),
            Err(Injected::Barrier(9))
        );
    }
}
