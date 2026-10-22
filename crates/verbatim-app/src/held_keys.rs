//! Caret keys the keyboard hook held back, passed on to the application
//! once Core has read the caret before them (`docs/design/key-holding.md`).
//!
//! The hook reports a held key with the stroke it came from
//! (`verbatim_input_windows::KeyReport::Held`); the reducer thread keeps
//! the stroke under the key's trace until Core asks for the key to be
//! passed on (`Effect::PassHeldKey`), then sends it with `SendInput`,
//! tagged `PASSED_ON_TAG` so the hook lets it through untouched. This is
//! NVDA's `KeyboardInputGesture.send` (`nvda/source/keyboardHandler.py`):
//! the gesture's modifiers not already down are pressed first and released
//! after, so a key whose Shift or Control the user let go of while its
//! caret was read still reaches the application as pressed; a key Windows
//! still has down is released first.

use std::collections::HashMap;

use verbatim_input_windows::{HeldStroke, PASSED_ON_TAG};
use verbatim_model::{GestureId, TraceId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY,
};

/// A held key waiting to be passed on.
struct Waiting {
    stroke: HeldStroke,
    /// The virtual-key codes of the gesture's modifiers: Shift, Control,
    /// Alt.
    modifiers: Vec<u16>,
    /// When the hook saw it, in microseconds since the Unix epoch.
    pressed_at_us: u64,
}

/// The held keys waiting to be passed on, by trace. Every one is passed on
/// once Core says so, which it does for every held key, so the table only
/// holds the keys whose caret is being read or which wait behind one.
#[derive(Default)]
pub(crate) struct HeldKeys {
    waiting: HashMap<TraceId, Waiting>,
}

impl HeldKeys {
    /// Keeps the key held as `gesture` from `stroke` until it is passed on.
    pub(crate) fn held(
        &mut self,
        trace_id: TraceId,
        gesture: &GestureId,
        stroke: HeldStroke,
        pressed_at_us: u64,
    ) {
        self.waiting.insert(
            trace_id,
            Waiting {
                stroke,
                modifiers: modifiers_of(gesture),
                pressed_at_us,
            },
        );
    }

    /// Sends the key held under `trace_id` to the application, and logs how
    /// long it was held, from the hook's press to the send.
    pub(crate) fn pass_on(&mut self, trace_id: TraceId) {
        let Some(waiting) = self.waiting.remove(&trace_id) else {
            tracing::warn!(%trace_id, "a held key to pass on was not held");
            return;
        };
        send(&waiting);
        let held_us = verbatim_outpost::protocol::now_us().saturating_sub(waiting.pressed_at_us);
        tracing::info!(
            target: "verbatim::held_key",
            %trace_id,
            held_us,
            "held key passed on"
        );
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

/// A keyboard input carrying [`PASSED_ON_TAG`].
fn key(vk: u16, scan: u32, extended: bool, up: bool) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if extended {
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
                wScan: u16::try_from(scan).unwrap_or(0),
                dwFlags: flags,
                time: 0,
                dwExtraInfo: PASSED_ON_TAG,
            },
        },
    }
}

/// Whether Windows has `vk` down now.
fn is_down(vk: u16) -> bool {
    // SAFETY: GetAsyncKeyState takes any virtual-key code.
    let state = unsafe { GetAsyncKeyState(i32::from(vk)) };
    state < 0
}

/// Sends the held key, as NVDA's gesture send does: its modifiers that are
/// not down pressed first and released last, and the key itself released
/// first if Windows still has it down.
fn send(waiting: &Waiting) {
    let stroke = waiting.stroke;
    let modifiers: Vec<u16> = waiting
        .modifiers
        .iter()
        .copied()
        .filter(|&vk| !is_down(vk))
        .collect();
    let mut inputs = Vec::with_capacity(2 * modifiers.len() + 3);
    if is_down(stroke.vk) {
        inputs.push(key(stroke.vk, stroke.scan_code, stroke.extended, true));
    }
    for &vk in &modifiers {
        inputs.push(key(vk, 0, false, false));
    }
    inputs.push(key(stroke.vk, stroke.scan_code, stroke.extended, false));
    inputs.push(key(stroke.vk, stroke.scan_code, stroke.extended, true));
    for &vk in modifiers.iter().rev() {
        inputs.push(key(vk, 0, false, true));
    }
    tracing::debug!(
        modifiers = ?waiting.modifiers,
        sent_modifiers = ?modifiers,
        key = stroke.vk,
        events = inputs.len(),
        "passing a held key on"
    );
    let size = i32::try_from(size_of::<INPUT>()).unwrap_or(i32::MAX);
    // SAFETY: `inputs` is a valid slice of initialized INPUT values whose
    // size is passed as the API requires.
    let sent = unsafe { SendInput(&inputs, size) };
    if usize::try_from(sent).unwrap_or(0) != inputs.len() {
        tracing::warn!(
            sent,
            wanted = inputs.len(),
            error = %windows::core::Error::from_thread(),
            "a held key could not be passed on in full"
        );
    }
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
