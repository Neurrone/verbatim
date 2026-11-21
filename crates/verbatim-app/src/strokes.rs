//! Sends the key strokes Core's stroke line names
//! (`books/dev/src/reference/crates/verbatim-core.md`, "The caret key on the event queue"). The
//! shell keeps no line of its own: the hook
//! reports a held caret key with its stroke and every transition it
//! deferred behind one, Core keeps them in its line, and names what goes,
//! in line order ([`verbatim_model::request::QueueEffect::SendStrokes`]),
//! which is sent here with one `SendInput`, each key tagged with its place
//! in line (`passed_on_info`) so the hook lets it through and knows it has
//! gone.
//!
//! At exit, once the hook is gone, Core names the keys whose last
//! transition in its line is a release
//! ([`verbatim_model::request::QueueEffect::ReleaseAtExit`]); each Windows
//! still has down is released, so no key is left down.
//!
//! A held key is sent as NVDA's `KeyboardInputGesture.send`
//! (`nvda/source/keyboardHandler.py`) sends a gesture: the gesture's
//! modifiers not already down are pressed first and released after, and a
//! key Windows still has down is released first. Since every transition
//! after a held key waits behind it, the modifiers are as they were when
//! it was pressed, so this sends extra modifiers only for a key whose
//! modifiers went down before the hook could see them.

use verbatim_input_windows::passed_on_info;
use verbatim_model::GestureId;
use verbatim_model::stroke::{HeldStroke, Modifier, SentTransition, Stroke};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, SendInput, VIRTUAL_KEY,
};

/// The caret key the hook held as `gesture`, its stroke `stroke`, with the
/// gesture's modifiers.
pub(crate) fn held_of(gesture: &GestureId, stroke: Stroke) -> HeldStroke {
    HeldStroke {
        stroke,
        modifiers: modifiers_of(gesture),
    }
}

/// Sends `transitions`, in order, with one `SendInput`, which Windows never
/// interleaves with other input.
pub(crate) fn send(transitions: &[SentTransition]) {
    let mut inputs = Vec::new();
    for transition in transitions {
        match transition {
            SentTransition::Held(held) => held_inputs(held, &mut inputs),
            SentTransition::Deferred { stroke, pressed } => {
                inputs.push(key(*stroke, !*pressed, stroke.sequence));
            }
        }
    }
    send_inputs(&inputs);
}

/// Releases each of `strokes`, which Core names at exit, that Windows still
/// has down, in order, and sends nothing else.
pub(crate) fn release_at_exit(strokes: &[Stroke]) {
    let inputs: Vec<INPUT> = still_down(strokes, is_down)
        .map(|stroke| key(stroke, true, stroke.sequence))
        .collect();
    tracing::info!(
        named = strokes.len(),
        released = inputs.len(),
        "keys released at exit"
    );
    send_inputs(&inputs);
}

/// The strokes of `strokes` that `down` says Windows has down, in order.
fn still_down(strokes: &[Stroke], down: impl Fn(u16) -> bool) -> impl Iterator<Item = Stroke> {
    strokes
        .iter()
        .copied()
        .filter(move |stroke| !stroke.is_unicode() && down(stroke.vk))
}

/// Sends `inputs` with one `SendInput`.
fn send_inputs(inputs: &[INPUT]) {
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

/// `gesture`'s Shift, Control and Alt, in that order. A normalized
/// identifier's parts are sorted, so the key is not always last; no caret
/// key is itself a modifier.
fn modifiers_of(gesture: &GestureId) -> Vec<Modifier> {
    let name = gesture.as_str();
    let name = name.strip_prefix("kb:").unwrap_or(name);
    let mut modifiers: Vec<Modifier> = name
        .split('+')
        .filter_map(|part| match part {
            "shift" => Some(Modifier::Shift),
            "control" => Some(Modifier::Control),
            "alt" => Some(Modifier::Alt),
            _ => None,
        })
        .collect();
    modifiers.sort_unstable();
    modifiers
}

/// A modifier's virtual-key code.
fn modifier_vk(modifier: Modifier) -> u16 {
    match modifier {
        Modifier::Shift => 0x10,
        Modifier::Control => 0x11,
        Modifier::Alt => 0x12,
    }
}

/// A keyboard input for `stroke`, a release when `up`, carrying place
/// `sequence` (0 for the modifiers and releases sent around a held key).
fn key(stroke: Stroke, up: bool, sequence: u32) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    let mut vk = stroke.vk;
    if stroke.is_unicode() {
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
fn modifier(vk: u16) -> Stroke {
    Stroke {
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
fn held_inputs(held: &HeldStroke, inputs: &mut Vec<INPUT>) {
    let stroke = held.stroke;
    let modifiers: Vec<u16> = held
        .modifiers
        .iter()
        .map(|&modifier| modifier_vk(modifier))
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
        modifiers = ?held.modifiers,
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
        let of = |name: &str| modifiers_of(&GestureId::parse(name).expect("a gesture"));
        assert_eq!(of("kb:rightarrow"), Vec::<Modifier>::new());
        assert_eq!(
            of("kb:shift+control+leftarrow"),
            [Modifier::Shift, Modifier::Control]
        );
        assert_eq!(of("kb:control+backspace"), [Modifier::Control]);
    }

    fn stroke(vk: u16, sequence: u32) -> Stroke {
        Stroke {
            vk,
            scan_code: 0,
            extended: false,
            sequence,
        }
    }

    /// Of the keys Core names at exit, a held key whose press reached
    /// Windows before it was held, its release swallowed with the held
    /// press, is still down and is released; one whose press never reached
    /// Windows is not down, and is not released; a character typed as
    /// Unicode leaves no key down.
    #[test]
    fn of_the_keys_core_names_at_exit_only_those_windows_has_down_are_released() {
        const RIGHT: u16 = 0x27;
        const DOWN: u16 = 0x28;
        let named = [
            stroke(RIGHT, 1),
            stroke(DOWN, 2),
            stroke(verbatim_model::stroke::VK_PACKET, 3),
        ];
        let down = |vk| vk == RIGHT || vk == verbatim_model::stroke::VK_PACKET;
        assert_eq!(
            still_down(&named, down).collect::<Vec<_>>(),
            [stroke(RIGHT, 1)]
        );
    }
}
