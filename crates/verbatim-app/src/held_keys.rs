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
        if inputs.is_empty() {
            return;
        }
        let size = i32::try_from(size_of::<INPUT>()).unwrap_or(i32::MAX);
        // SAFETY: `inputs` is a valid slice of initialized INPUT values whose
        // size is passed as the API requires.
        let sent = unsafe { SendInput(&inputs, size) };
        if usize::try_from(sent).unwrap_or(0) != inputs.len() {
            tracing::warn!(
                sent,
                wanted = inputs.len(),
                error = %windows::core::Error::from_thread(),
                "keys held back could not be passed on in full"
            );
        }
    }
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
}
