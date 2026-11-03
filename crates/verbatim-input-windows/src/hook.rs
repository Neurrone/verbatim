//! The code that runs on the hook thread: the hook procedure, which gathers
//! each key's inputs, has the decision machine decide it, and carries out
//! its verdict, and the thread's one message loop
//! (`docs/design/ownership.md`).
//!
//! Nothing here blocks or takes a lock: `cargo xtask ci` checks that this
//! module names nothing that does (`xtask/src/ownership.rs`, the hook
//! rule). Everything a key causes leaves through the one report callback,
//! in the order the thread handled it, and the thread numbers every key
//! press ([`KeySeq`]) in that same order.

use std::cell::RefCell;
use std::sync::Arc;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_NUMLOCK};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetForegroundWindow, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED,
    LLKHF_INJECTED, MSG, WM_KEYDOWN, WM_SYSKEYDOWN,
};

use verbatim_input::state::{
    DecisionMachine, HoldPermit, KeyInput, KeySource, PressReport, VK_MASK, Verdict,
};
use verbatim_input::{KeyDecision, KeyEvent};
use verbatim_model::{HookForeground, KeySeq, TraceId};

use crate::{
    Effects, ForegroundView, HeldStroke, KeyReport, KeyReportFn, OWN_INPUT_TAG, SpeechEffectFn,
    passed_on_sequence, typed,
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
    /// The last key number given out, to a key press: this thread is the
    /// one minter of key numbers.
    last_key: KeySeq,
    /// Core's view of the foreground window ([`ForegroundView`]).
    foreground: Arc<ForegroundView>,
    /// Reads the window in front: `GetForegroundWindow`, a local read, or
    /// a fixed window in tests.
    front: fn() -> HWND,
}

impl HookState {
    /// A hook state with nothing held, deciding with `machine`.
    pub(crate) fn new(
        machine: DecisionMachine,
        (speech, reports, foreground): Effects,
        typing: typed::Typing,
    ) -> Self {
        Self {
            machine,
            speech,
            reports,
            typing,
            last_key: KeySeq::NONE_YET,
            foreground,
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
/// error) ends it. A low-level hook only fires while its installing thread
/// pumps messages, and Windows calls the hook procedure from within this
/// loop's waits, so keys are handled one at a time, in the one order the
/// thread takes them. The thread takes no other message: any posted to it
/// is left alone. It runs no other loop while the hook is installed.
pub(crate) fn pump_messages() {
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

/// What Core's `view` lets the machine do with a key, and the window in
/// front when it was read for that. The window is read only when the view
/// lets keys be held and either the focus has text or a key waits in the
/// machine's line (`outstanding`); otherwise the permit is
/// [`HoldPermit::None`] with no call. The view applies only while the
/// window it names is still in front: until Core catches up with a
/// change, no key is held or deferred. A window whose process has higher
/// integrity than Verbatim's is one where keys may not be held, since
/// Windows would drop a key sent again to it ([`ForegroundView`]).
fn permit(
    view: Option<HookForeground>,
    outstanding: bool,
    front: fn() -> HWND,
) -> (HoldPermit, Option<HWND>) {
    let Some(view) = view.filter(|view| view.holds_keys && (view.focus_has_text || outstanding))
    else {
        return (HoldPermit::None, None);
    };
    let window = front();
    let permit = if typed::window_handle(window) != view.window {
        HoldPermit::None
    } else if view.focus_has_text {
        HoldPermit::HoldAndDefer
    } else {
        HoldPermit::Defer
    };
    (permit, Some(window))
}

/// Reports what a key press that reaches the application, now or once
/// deferred, did: the text it types, or, for a key that types nothing and
/// may have edited the text ([`PressReport::TranslatedOrPassed`]), that it
/// was passed. `foreground` is Core's view and the window in front, when
/// it was read for the key's permit.
fn report_passed(
    state: &mut HookState,
    (vk, scan_code): (u16, u32),
    key_seq: KeySeq,
    typing: PressReport,
    pressed_at_us: u64,
    (view, window): (Option<HookForeground>, Option<HWND>),
) {
    let trace_id = TraceId::mint();
    let deferred: Vec<(u16, bool)> = state.machine.deferred_transitions().collect();
    let front = state.front;
    match state.typing.translate(
        (vk, scan_code),
        &deferred,
        (|| window.unwrap_or_else(front), view),
    ) {
        Some(text) => (state.reports)(KeyReport::Typed {
            trace_id,
            key_seq,
            text,
            pressed_at_us,
        }),
        None if typing == PressReport::TranslatedOrPassed => {
            (state.reports)(KeyReport::Passed {
                trace_id,
                key_seq,
                pressed_at_us,
            });
        }
        None => {}
    }
}

/// Reports a gesture's mask, if it has one, after the gesture: the unknown
/// key pressed and released, from its places in line, sent on as any
/// deferred transition.
fn report_mask(state: &HookState, mask: Option<(u32, u32)>) {
    let Some((press, release)) = mask else {
        return;
    };
    for (sequence, pressed) in [(press, true), (release, false)] {
        (state.reports)(KeyReport::Deferred {
            stroke: HeldStroke {
                vk: VK_MASK,
                scan_code: 0,
                extended: false,
                sequence,
            },
            pressed,
        });
    }
}

/// Gathers one key transition's inputs, has the machine decide it, carries
/// out its effect on speech, and reports what it caused, each report
/// carrying the key's number; `scan_code` and `extra_info` are the key
/// event's own.
pub(crate) fn decide(
    state: &mut HookState,
    event: KeyEvent,
    scan_code: u32,
    extra_info: usize,
) -> KeyDecision {
    // A key Verbatim passes on, with the modifiers sent around it, goes
    // straight to the application, reading nothing: it was decided and
    // reported when it was pressed. Its place takes the line up to it.
    if let Some(place) = passed_on_sequence(extra_info) {
        state.machine.passed_on(place);
        return KeyDecision::Pass;
    }
    let source = if extra_info == OWN_INPUT_TAG {
        KeySource::Own
    } else {
        KeySource::User
    };
    // Num Lock decides what the numpad's operator keys are: the
    // keyboard's own state, read locally before each key.
    // SAFETY: GetKeyState takes any virtual-key code.
    let num_lock = unsafe { GetKeyState(i32::from(VK_NUMLOCK.0)) } & 1 != 0;
    // The key's time, which the machine decides by. Only a passed key's
    // report reuses it; the other reports read the time again.
    let now_us = unix_us();
    let view = state.foreground.get();
    let (permit, window) = permit(view, state.machine.has_outstanding(), state.front);
    let decision = state.machine.on_key(KeyInput {
        event,
        now: now_us,
        num_lock,
        source,
        permit,
        // From the view alone, reading no window: a mask decided for a
        // window no longer in front is dropped by Windows or sent to the
        // new one, and every transition behind it reads the window in
        // front for its own permit, since the line is then not empty.
        may_mask: view.is_some_and(|view| view.holds_keys),
    });
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
    if let Some(effect) = decision.speech {
        (state.speech)(effect, key_seq);
    }
    if let Some(emitted) = decision.emitted {
        (state.reports)(KeyReport::Gesture {
            gesture: emitted.stamped(TraceId::mint()),
            key_seq,
            pressed_at_us: unix_us(),
        });
    }
    if let Some(observed) = decision.observed {
        let pressed_at_us = unix_us();
        let observed = observed.stamped(TraceId::mint());
        (state.reports)(if let Verdict::Hold(sequence) = decision.verdict {
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
    if decision.typing != PressReport::Nothing {
        report_passed(
            state,
            (event.vk, scan_code),
            key_seq,
            decision.typing,
            now_us,
            (view, window),
        );
    }
    // After what the key did, so that whoever passes it on has been told
    // of it first.
    if let Verdict::Defer(sequence) = decision.verdict {
        (state.reports)(KeyReport::Deferred {
            stroke: stroke(sequence),
            pressed: event.pressed,
        });
    }
    report_mask(state, decision.mask);
    // A lock key reaching the operating system is reported, for its new
    // state to be announced, as NVDA announces it.
    if let Some(toggle) = decision.toggle {
        (state.reports)(KeyReport::Gesture {
            gesture: verbatim_input::state::EmittedGesture {
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
    if decision.verdict == Verdict::Pass {
        KeyDecision::Pass
    } else {
        KeyDecision::Swallow
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use verbatim_input::GestureMap;
    use verbatim_input::state::DecisionConfig;
    use verbatim_model::{GestureId, HookForeground, WindowHandle};

    use super::*;
    use crate::{PASSED_ON_TAG, passed_on_info};

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
            DecisionMachine::new(DecisionConfig::default(), map),
            (
                Box::new(|_, _| {}),
                Box::new(move |report| sink.lock().unwrap().push(report)),
                Arc::new(ForegroundView::new()),
            ),
            typed::Typing::default(),
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

    /// A hook state observing Escape, holding Right Arrow, whose reports
    /// are collected in `reports`, with Core's view of the window in front
    /// letting keys be held and the focus having text.
    fn holding_state(reports: &Arc<Mutex<Vec<KeyReport>>>) -> HookState {
        let right = GestureId::parse("kb:rightarrow").expect("valid id");
        let escape = GestureId::parse("kb:escape").expect("valid id");
        let map = GestureMap::new([])
            .with_observed([escape])
            .with_held([right]);
        let sink = Arc::clone(reports);
        let foreground = Arc::new(ForegroundView::new());
        foreground.set(Some(IN_FRONT));
        let mut state = HookState::new(
            DecisionMachine::new(DecisionConfig::default(), map),
            (
                Box::new(|_, _| {}),
                Box::new(move |report| sink.lock().unwrap().push(report)),
                foreground,
            ),
            typed::Typing::default(),
        );
        state.front = window_in_front;
        state
    }

    /// The window the hook's tests have in front: not a real window, so
    /// that nothing depends on the desktop the tests run on; its keys are
    /// translated with the test thread's own keyboard layout.
    fn window_in_front() -> HWND {
        HWND(7 as *mut core::ffi::c_void)
    }

    /// Core's view of [`window_in_front`], whose keys may be held, with
    /// the focus in text.
    const IN_FRONT: HookForeground = HookForeground {
        window: WindowHandle(7),
        holds_keys: true,
        focus_has_text: true,
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
            })
            .collect()
    }

    /// While Core's view of the window in front says keys may not be held
    /// for it, as for a window of higher integrity than Verbatim's, a caret
    /// key passes unheld and is reported observed, and no key is deferred
    /// behind a key held before the window came to the front, whose
    /// release is still held back with it.
    #[test]
    fn no_key_is_held_where_core_says_none_may_be() {
        let reports = Arc::new(Mutex::new(Vec::new()));
        let mut state = holding_state(&reports);
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
    /// before Core has any view, no key is held; nor while the focus has
    /// no text.
    #[test]
    fn no_key_is_held_until_core_has_a_view_of_the_window_in_front() {
        let reports = Arc::new(Mutex::new(Vec::new()));
        let mut state = holding_state(&reports);
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
        state.foreground.set(Some(HookForeground {
            focus_has_text: false,
            ..IN_FRONT
        }));
        assert_eq!(
            decide(&mut state, right_arrow(true), 0x4D, 0),
            KeyDecision::Pass
        );
        assert_eq!(
            described(&reports.lock().unwrap()),
            [
                "observed kb:rightarrow",
                "observed kb:rightarrow",
                "observed kb:rightarrow"
            ]
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

    /// A key carrying [`PASSED_ON_TAG`], a held key Verbatim passes on,
    /// passes straight through, reported to nobody, though holding is on;
    /// a key the end-to-end harness injects, with its stroke number, is the
    /// user's and is held, and its stroke's end reported behind it.
    #[test]
    fn a_key_passed_on_passes_untouched_and_a_harness_key_is_held() {
        let reports = Arc::new(Mutex::new(Vec::new()));
        let mut state = holding_state(&reports);
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

    /// Verbatim+Alt+Home in the order the end-to-end agent sends it in one
    /// batch (Insert, Alt and Home down, then up in reverse), with Core's
    /// view letting keys be held and the focus without text: the gesture
    /// is reported, then its mask, the unknown key pressed and released,
    /// and Alt's release, which reaches the hook before anything is sent
    /// back, is deferred behind the mask. Once the three come back passed
    /// on, a second round is the same, its gesture counted as a double
    /// press.
    #[test]
    fn a_gesture_with_alt_down_is_masked_and_alts_release_waits_behind_the_mask() {
        let reports = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&reports);
        let foreground = Arc::new(ForegroundView::new());
        foreground.set(Some(HookForeground {
            focus_has_text: false,
            ..IN_FRONT
        }));
        let map = GestureMap::new([GestureId::parse("kb:verbatim+alt+home").expect("valid id")]);
        let mut state = HookState::new(
            DecisionMachine::new(DecisionConfig::default(), map),
            (
                Box::new(|_, _| {}),
                Box::new(move |report| sink.lock().unwrap().push(report)),
                foreground,
            ),
            typed::Typing::default(),
        );
        state.front = window_in_front;
        let key = |vk, scan_code, extended, pressed| KeyEvent {
            vk,
            scan_code,
            extended,
            injected: true,
            pressed,
        };
        let insert = |pressed| key(0x2D, 0x52, true, pressed);
        let alt = |pressed| key(0xA4, 0x38, false, pressed);
        let home = |pressed| key(0x24, 0x47, true, pressed);
        let (swallow, pass) = (KeyDecision::Swallow, KeyDecision::Pass);
        for first in [1, 4] {
            let mut decided = Vec::new();
            for (event, extra_info) in [
                (insert(true), 0),
                (alt(true), 0),
                (home(true), 0),
                (home(false), 0),
                (alt(false), 0),
                (insert(false), 0),
                (key(0xFF, 0, false, true), passed_on_info(first)),
                (key(0xFF, 0, false, false), passed_on_info(first + 1)),
                (alt(false), passed_on_info(first + 2)),
            ] {
                decided.push(decide(&mut state, event, event.scan_code, extra_info));
            }
            assert_eq!(
                decided,
                [
                    swallow, pass, swallow, swallow, swallow, swallow, pass, pass, pass
                ]
            );
        }
        let reports = reports.lock().unwrap();
        assert_eq!(
            described(&reports),
            [
                "gesture kb:alt+home+verbatim",
                "deferred 0xff down #1",
                "deferred 0xff up #2",
                "deferred 0xa4 up #3",
                "gesture kb:alt+home+verbatim",
                "deferred 0xff down #4",
                "deferred 0xff up #5",
                "deferred 0xa4 up #6",
            ]
        );
        let repeats: Vec<u8> = reports
            .iter()
            .filter_map(|report| match report {
                KeyReport::Gesture { gesture, .. } => Some(gesture.repeat),
                _ => None,
            })
            .collect();
        assert_eq!(repeats, [0, 1]);
    }

    /// Each verdict is carried out: a held key is swallowed and reported
    /// held with its stroke; a deferred one swallowed and reported, after
    /// what it typed; a key swallowed for good, the Verbatim modifier's
    /// press and release, reports nothing of its own;
    /// a key passed goes down the hook chain, with what it did reported;
    /// and a lock key passed reports its gesture.
    #[test]
    fn each_verdict_is_carried_out() {
        let reports = Arc::new(Mutex::new(Vec::new()));
        let mut state = holding_state(&reports);
        let key = |vk, scan_code, extended| KeyEvent {
            vk,
            scan_code,
            extended,
            injected: false,
            pressed: true,
        };
        let mut decided = Vec::new();
        for (event, extra_info) in [
            (right_arrow(true), 0),
            (letter_x(true), 0),
            (key(0x2D, 0x52, true), 0),
            (
                KeyEvent {
                    pressed: false,
                    ..key(0x2D, 0x52, true)
                },
                0,
            ),
            (key(0x58, 0x2D, false), passed_on_info(2)),
            (key(0x1B, 0x01, false), 0),
            (key(0x72, 0x3D, false), 0),
            (key(0x91, 0x46, false), 0),
        ] {
            decided.push(decide(&mut state, event, event.scan_code, extra_info));
        }
        let (swallow, pass) = (KeyDecision::Swallow, KeyDecision::Pass);
        assert_eq!(
            decided,
            [swallow, swallow, swallow, swallow, pass, pass, pass, pass]
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
                "typed x",
                "deferred 0x58 down #2",
                "observed kb:escape",
                "passed",
                "gesture kb:scrolllock",
            ]
        );
    }
}
