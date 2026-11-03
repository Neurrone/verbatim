//! Keys the keyboard hook held back, passed on to the application in the
//! order pressed (`docs/design/key-holding.md`).
//!
//! The hook reports a held caret key with the stroke it came from
//! (`verbatim_input_windows::KeyReport::Held`), and every key transition
//! it deferred behind a held key (`KeyReport::Deferred`). The reducer
//! thread keeps them in one line, in the order the hook reported them. A
//! held key waits at the front until Core asks for it to be passed on
//! (`Effect::PassHeldKey`), once its caret has been read; a deferred
//! transition is never read, and goes as soon as it reaches the front.
//! Whatever reaches the front is sent with one `SendInput`, each key
//! tagged with its place in line (`passed_on_info`) so the hook lets it
//! through and knows it has gone.
//!
//! At exit, once the hook is gone, what is still in line is dropped, but
//! no key is left down: a key released behind a held key, whose press
//! reached the application before it, is released ([`HeldKeys::exit`]).
//!
//! A held key is sent as NVDA's `KeyboardInputGesture.send`
//! (`nvda/source/keyboardHandler.py`) sends a gesture: the gesture's
//! modifiers not already down are pressed first and released after, and a
//! key Windows still has down is released first. Since every transition
//! after a held key waits behind it, the modifiers are as they were when
//! it was pressed, so this sends extra modifiers only for a key whose
//! modifiers went down before the hook could see them.

use std::collections::VecDeque;

use verbatim_input_windows::{HeldStroke, passed_on_info};
use verbatim_model::{GestureId, TraceId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, SendInput, VIRTUAL_KEY,
};

/// `VK_PACKET`: a character typed as Unicode, its UTF-16 unit in the scan
/// code.
const VK_PACKET: u16 = 0xE7;

/// A key in line.
enum Waiting {
    /// A held caret key, sent once Core passes it on.
    Held {
        trace_id: TraceId,
        stroke: HeldStroke,
        /// The virtual-key codes of the gesture's modifiers: Shift,
        /// Control, Alt.
        modifiers: Vec<u16>,
        /// When the hook saw it, in microseconds since the Unix epoch.
        pressed_at_us: u64,
        /// Whether Core has passed it on.
        passed: bool,
    },
    /// A transition deferred behind a held key.
    Deferred { stroke: HeldStroke, pressed: bool },
}

/// The keys held back, in the order the hook reported them. Core passes on
/// every held key, so the line holds only keys whose caret is being read
/// and the keys behind them.
#[derive(Default)]
pub(crate) struct HeldKeys {
    line: VecDeque<Waiting>,
}

impl HeldKeys {
    /// Keeps the key held as `gesture` from `stroke` until Core passes it
    /// on.
    pub(crate) fn held(
        &mut self,
        trace_id: TraceId,
        gesture: &GestureId,
        stroke: HeldStroke,
        pressed_at_us: u64,
    ) {
        self.line.push_back(Waiting::Held {
            trace_id,
            stroke,
            modifiers: modifiers_of(gesture),
            pressed_at_us,
            passed: false,
        });
    }

    /// Whether no key waits to be passed on.
    pub(crate) fn is_empty(&self) -> bool {
        self.line.is_empty()
    }

    /// Puts a deferred transition in line, and sends it at once if nothing
    /// waits ahead of it.
    pub(crate) fn deferred(&mut self, stroke: HeldStroke, pressed: bool) {
        self.line.push_back(Waiting::Deferred { stroke, pressed });
        self.send_ready();
    }

    /// Passes on the key held under `trace_id`, with every deferred key
    /// behind it up to the next held key still waiting, and logs how long
    /// it was held, from the hook's press to the send.
    pub(crate) fn pass_on(&mut self, trace_id: TraceId) {
        let Some(passed) = self.line.iter_mut().find_map(|waiting| match waiting {
            Waiting::Held {
                trace_id: held,
                passed,
                ..
            } if *held == trace_id => Some(passed),
            _ => None,
        }) else {
            tracing::warn!(%trace_id, "a held key to pass on was not held");
            return;
        };
        *passed = true;
        self.send_ready();
    }

    /// Verbatim is exiting and the hook is gone: the keys still in line are
    /// dropped, as they would arrive after keys pressed since, but no key is
    /// left down by the line. Each key whose last transition in line is a
    /// release, and which Windows still has down because its press reached
    /// the application before the release was deferred, is released.
    pub(crate) fn exit(&mut self) {
        let line = std::mem::take(&mut self.line);
        let inputs: Vec<INPUT> = releases_at_exit(&line, is_down)
            .into_iter()
            .map(|stroke| key(stroke, true, stroke.sequence))
            .collect();
        if !line.is_empty() {
            tracing::info!(
                dropped = line.len(),
                released = inputs.len(),
                "keys held back dropped at exit"
            );
        }
        send(&inputs);
    }

    /// Sends, with one `SendInput`, every key at the front of the line that
    /// may go: deferred transitions, and held keys Core has passed on.
    fn send_ready(&mut self) {
        let mut inputs = Vec::new();
        while let Some(front) = self.line.front() {
            match front {
                Waiting::Held { passed: false, .. } => break,
                Waiting::Held {
                    trace_id,
                    stroke,
                    modifiers,
                    pressed_at_us,
                    ..
                } => {
                    held_inputs(*stroke, modifiers, &mut inputs);
                    let held_us = verbatim_process::clock::now_us().saturating_sub(*pressed_at_us);
                    tracing::info!(
                        target: "verbatim::held_key",
                        %trace_id,
                        held_us,
                        "held key passed on"
                    );
                }
                Waiting::Deferred { stroke, pressed } => {
                    inputs.push(key(*stroke, !*pressed, stroke.sequence));
                }
            }
            self.line.pop_front();
        }
        send(&inputs);
    }
}

/// Sends `inputs` with one `SendInput`, which Windows never interleaves
/// with other input.
fn send(inputs: &[INPUT]) {
    if inputs.is_empty() {
        return;
    }
    let size = i32::try_from(size_of::<INPUT>()).unwrap_or(i32::MAX);
    // SAFETY: `inputs` is a valid slice of initialized INPUT values whose
    // size is passed as the API requires.
    let sent = unsafe { SendInput(inputs, size) };
    if usize::try_from(sent).unwrap_or(0) != inputs.len() {
        tracing::warn!(
            sent,
            wanted = inputs.len(),
            error = %windows::core::Error::from_thread(),
            "keys held back could not be passed on in full"
        );
    }
}

/// The releases to send at exit for the keys in `line`, which is dropped:
/// one for each key whose last transition in line is a release and which
/// `down` says Windows has down, in the order of those releases. A held key
/// counts as a release, since passing it on sends its press and its
/// release: its own release is swallowed for good, so Windows has it down
/// when an earlier press of it reached Windows, and is released, and not
/// when none did. A key whose last transition is a deferred press is still
/// down under the user's finger, and its release, which the hook no longer
/// sees, reaches Windows itself. A character typed as Unicode leaves no
/// key down.
fn releases_at_exit(line: &VecDeque<Waiting>, down: impl Fn(u16) -> bool) -> Vec<HeldStroke> {
    let mut last: Vec<(HeldStroke, bool)> = Vec::new();
    for waiting in line {
        let (stroke, pressed) = match waiting {
            Waiting::Held { stroke, .. } => (stroke, &false),
            Waiting::Deferred { stroke, pressed } => (stroke, pressed),
        };
        if stroke.vk == VK_PACKET {
            continue;
        }
        // Moved to the end, so the releases go in the order of the last.
        last.retain(|(earlier, _)| earlier.vk != stroke.vk);
        last.push((*stroke, *pressed));
    }
    last.into_iter()
        .filter(|&(stroke, pressed)| !pressed && down(stroke.vk))
        .map(|(stroke, _)| stroke)
        .collect()
}

/// The virtual-key codes of `gesture`'s Shift, Control and Alt, in that
/// order. A normalized identifier's parts are sorted, so the key is not
/// always last; no caret key is itself a modifier.
fn modifiers_of(gesture: &GestureId) -> Vec<u16> {
    let name = gesture.as_str();
    let name = name.strip_prefix("kb:").unwrap_or(name);
    let mut modifiers: Vec<u16> = name
        .split('+')
        .filter_map(|part| match part {
            "shift" => Some(0x10),
            "control" => Some(0x11),
            "alt" => Some(0x12),
            _ => None,
        })
        .collect();
    modifiers.sort_unstable();
    modifiers
}

/// A keyboard input for `stroke`, a release when `up`, carrying place
/// `sequence` (0 for the modifiers and releases sent around a held key).
fn key(stroke: HeldStroke, up: bool, sequence: u32) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    let mut vk = stroke.vk;
    if stroke.vk == VK_PACKET {
        flags |= KEYEVENTF_UNICODE;
        vk = 0;
    }
    if stroke.extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: u16::try_from(stroke.scan_code).unwrap_or(0),
                dwFlags: flags,
                time: 0,
                dwExtraInfo: passed_on_info(sequence),
            },
        },
    }
}

/// A modifier key's stroke, with no scan code.
fn modifier(vk: u16) -> HeldStroke {
    HeldStroke {
        vk,
        scan_code: 0,
        extended: false,
        sequence: 0,
    }
}

/// Whether Windows has `vk` down now.
fn is_down(vk: u16) -> bool {
    // SAFETY: GetAsyncKeyState takes any virtual-key code.
    let state = unsafe { GetAsyncKeyState(i32::from(vk)) };
    state < 0
}

/// Adds the inputs that send a held key to `inputs`, as NVDA's gesture
/// send does: its modifiers that are not down pressed first and released
/// last, and the key itself released first if Windows still has it down.
/// The last input carries the key's place in line, so the hook takes the
/// key as gone only once all of it has come through.
fn held_inputs(stroke: HeldStroke, gesture_modifiers: &[u16], inputs: &mut Vec<INPUT>) {
    let modifiers: Vec<u16> = gesture_modifiers
        .iter()
        .copied()
        .filter(|&vk| !is_down(vk))
        .collect();
    // Each transition as a stroke and whether it is a release.
    let mut transitions = Vec::with_capacity(2 * modifiers.len() + 3);
    if is_down(stroke.vk) {
        transitions.push((stroke, true));
    }
    transitions.extend(modifiers.iter().map(|&vk| (modifier(vk), false)));
    transitions.push((stroke, false));
    transitions.push((stroke, true));
    transitions.extend(modifiers.iter().rev().map(|&vk| (modifier(vk), true)));
    let last = transitions.len() - 1;
    inputs.extend(
        transitions
            .into_iter()
            .enumerate()
            .map(|(at, (sent, up))| key(sent, up, if at == last { stroke.sequence } else { 0 })),
    );
    tracing::debug!(
        modifiers = ?gesture_modifiers,
        sent_modifiers = ?modifiers,
        key = stroke.vk,
        events = last + 1,
        "passing a held key on"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gestures_modifiers_are_its_shift_control_and_alt() {
        let of = |id: &str| modifiers_of(&GestureId::parse(id).expect("valid id"));
        assert_eq!(of("kb:rightarrow"), Vec::<u16>::new());
        assert_eq!(of("kb:shift+control+leftarrow"), [0x10, 0x11]);
        assert_eq!(of("kb:control+backspace"), [0x11]);
    }

    fn stroke(vk: u16, sequence: u32) -> HeldStroke {
        HeldStroke {
            vk,
            scan_code: 0,
            extended: false,
            sequence,
        }
    }

    fn deferred(vk: u16, pressed: bool, sequence: u32) -> Waiting {
        Waiting::Deferred {
            stroke: stroke(vk, sequence),
            pressed,
        }
    }

    /// At exit the line is dropped, and the keys whose release waits in it
    /// with their press already in Windows are released: left Shift,
    /// pressed before a held Down Arrow and released behind it, its
    /// auto-repeat deferred too, and right Alt; not X, whose press was
    /// deferred and dropped with it, nor left Control, still down under the
    /// user's finger, nor Alt, which Windows does not have down, nor a
    /// character typed as Unicode.
    #[test]
    fn a_key_released_behind_a_held_key_is_released_at_exit() {
        const SHIFT: u16 = 0xA0;
        const CONTROL: u16 = 0xA2;
        const ALT: u16 = 0x12;
        const RIGHT_ALT: u16 = 0xA5;
        const X: u16 = 0x58;
        let line = VecDeque::from([
            Waiting::Held {
                trace_id: TraceId::from_raw(1),
                stroke: stroke(0x28, 1),
                modifiers: vec![0x10],
                pressed_at_us: 0,
                passed: false,
            },
            deferred(SHIFT, true, 2),
            deferred(X, true, 3),
            deferred(RIGHT_ALT, false, 4),
            deferred(SHIFT, false, 5),
            deferred(X, false, 6),
            deferred(CONTROL, true, 7),
            deferred(VK_PACKET, false, 8),
            deferred(ALT, false, 9),
        ]);
        let down = |vk| matches!(vk, SHIFT | CONTROL | RIGHT_ALT);
        assert_eq!(
            releases_at_exit(&line, down),
            [stroke(RIGHT_ALT, 4), stroke(SHIFT, 5)]
        );
    }

    /// A key released behind a held key and then pressed again is down
    /// under the user's finger at exit, so no release is sent for it; a key
    /// released twice is released once; and a character typed as Unicode
    /// is released by no key, even when Windows answers it is down.
    #[test]
    fn only_a_keys_last_transition_decides_its_release_at_exit() {
        const SHIFT: u16 = 0xA0;
        const CONTROL: u16 = 0xA2;
        let line = VecDeque::from([
            Waiting::Held {
                trace_id: TraceId::from_raw(1),
                stroke: stroke(0x28, 1),
                modifiers: Vec::new(),
                pressed_at_us: 0,
                passed: false,
            },
            deferred(SHIFT, false, 2),
            deferred(SHIFT, true, 3),
            deferred(CONTROL, false, 4),
            deferred(CONTROL, false, 5),
            deferred(VK_PACKET, false, 6),
        ]);
        let down = |vk| matches!(vk, SHIFT | CONTROL | VK_PACKET);
        assert_eq!(releases_at_exit(&line, down), [stroke(CONTROL, 5)]);
    }

    /// A held key whose earlier press reached Windows has its release
    /// swallowed with the held press, so Windows still has it down at exit
    /// and it is released; a held key whose press never reached Windows is
    /// not down, and is not released.
    #[test]
    fn a_held_key_windows_still_has_down_is_released_at_exit() {
        const RIGHT: u16 = 0x27;
        const DOWN: u16 = 0x28;
        let held = |vk, number: u32| Waiting::Held {
            trace_id: TraceId::from_raw(u64::from(number)),
            stroke: stroke(vk, number),
            modifiers: Vec::new(),
            pressed_at_us: 0,
            passed: false,
        };
        let line = VecDeque::from([held(RIGHT, 1), held(DOWN, 2)]);
        let down = |vk| vk == RIGHT;
        assert_eq!(releases_at_exit(&line, down), [stroke(RIGHT, 1)]);
    }
}
