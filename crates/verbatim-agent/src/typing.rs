//! Typing text as real key presses, for
//! [`Request::TypeText`](crate::protocol::Request::TypeText).
//!
//! Each character is looked up in the keyboard layout of the foreground
//! window's thread with `VkKeyScanEx`, which gives the virtual key that
//! types it and the Shift, Control, and Alt state it needs (Control with
//! Alt is Alt Gr). The keys are then pressed as a user would press them:
//! the modifiers down, the key down and up, the modifiers up, each event
//! carrying the key's scan code in that layout. A keyboard hook, such as
//! the screen reader's, therefore sees ordinary typing, which typed
//! character echo depends on. Every character is looked up before any key
//! is sent, so text with one character the layout cannot type sends
//! nothing.

use std::io;
use std::mem::size_of;

use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardLayout, HKL, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC, MapVirtualKeyExW, SendInput, VIRTUAL_KEY, VK_LCONTROL,
    VK_LMENU, VK_LSHIFT, VkKeyScanExW,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

/// The shift-state bits `VkKeyScanEx` reports in its high byte, and the
/// modifier key pressed for each, in the order they go down.
const MODIFIERS: [(u8, VIRTUAL_KEY); 3] = [(1, VK_LSHIFT), (2, VK_LCONTROL), (4, VK_LMENU)];

/// One key transition: a virtual key going down or coming up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stroke {
    key: VIRTUAL_KEY,
    up: bool,
}

/// Types `text` into whatever has the keyboard focus.
///
/// # Errors
///
/// Returns [`io::ErrorKind::InvalidInput`], having sent nothing, when a
/// character is a control character or the layout cannot type it, and an
/// error when `SendInput` injects fewer events than it was given (another
/// desktop, such as the secure desktop, has the input).
pub fn type_text(text: &str) -> io::Result<()> {
    let layout = foreground_layout();
    let strokes = strokes_for(text, |unit| {
        // SAFETY: a plain lookup taking a UTF-16 code unit and a layout
        // handle; an unknown layout makes it report no key.
        unsafe { VkKeyScanExW(unit, layout) }
    })
    .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;
    send(&strokes, layout)
}

/// The keyboard layout of the foreground window's thread, which is the
/// layout the application being typed into translates keys with; this
/// thread's own when there is no foreground window.
fn foreground_layout() -> HKL {
    // SAFETY: GetForegroundWindow has no preconditions.
    let window = unsafe { GetForegroundWindow() };
    // SAFETY: tolerates any window handle, returning 0 for an invalid one.
    let thread = unsafe { GetWindowThreadProcessId(window, None) };
    // SAFETY: takes any thread id; 0 names the calling thread.
    unsafe { GetKeyboardLayout(thread) }
}

/// The key transitions that type `text`, given `scan`, a `VkKeyScanEx`
/// lookup in the target layout. Pure, so it can be tested with a layout of
/// the test's own.
///
/// # Errors
///
/// Returns a message naming the first character that is a control
/// character, lies outside the Basic Multilingual Plane, has no key in the
/// layout, or needs a shift state other than Shift, Control, and Alt.
fn strokes_for(text: &str, scan: impl Fn(u16) -> i16) -> Result<Vec<Stroke>, String> {
    let mut strokes = Vec::new();
    for character in text.chars() {
        if character.is_control() {
            return Err(format!(
                "{character:?} is a control character; send named keys with SendKeys"
            ));
        }
        let mut units = [0u16; 2];
        let encoded = character.encode_utf16(&mut units);
        let [unit] = *encoded else {
            return Err(format!(
                "{character:?} cannot be typed as one key in any keyboard layout"
            ));
        };
        let [key, shift] = scan(unit).to_le_bytes();
        if key == 0xFF && shift == 0xFF {
            return Err(format!(
                "{character:?} has no key in the active keyboard layout"
            ));
        }
        if shift & !0b111 != 0 {
            return Err(format!(
                "{character:?} needs a shift state ({shift:#x}) other than Shift, Control, and Alt"
            ));
        }
        let held: Vec<VIRTUAL_KEY> = MODIFIERS
            .iter()
            .filter(|(bit, _)| shift & bit != 0)
            .map(|(_, modifier)| *modifier)
            .collect();
        let key = VIRTUAL_KEY(u16::from(key));
        strokes.extend(held.iter().map(|&modifier| Stroke {
            key: modifier,
            up: false,
        }));
        strokes.push(Stroke { key, up: false });
        strokes.push(Stroke { key, up: true });
        strokes.extend(held.iter().rev().map(|&modifier| Stroke {
            key: modifier,
            up: true,
        }));
    }
    Ok(strokes)
}

/// Injects `strokes` in one `SendInput` call, each with its scan code in
/// `layout`.
fn send(strokes: &[Stroke], layout: HKL) -> io::Result<()> {
    if strokes.is_empty() {
        return Ok(());
    }
    let inputs: Vec<INPUT> = strokes
        .iter()
        .map(|stroke| {
            // SAFETY: a plain lookup taking a virtual key and a layout
            // handle; it returns 0 when there is no scan code.
            let scan =
                unsafe { MapVirtualKeyExW(u32::from(stroke.key.0), MAPVK_VK_TO_VSC, Some(layout)) };
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: stroke.key,
                        wScan: u16::try_from(scan).unwrap_or(0),
                        dwFlags: if stroke.up {
                            KEYEVENTF_KEYUP
                        } else {
                            KEYBD_EVENT_FLAGS(0)
                        },
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            }
        })
        .collect();
    let input_size = i32::try_from(size_of::<INPUT>()).unwrap_or(i32::MAX);
    // SAFETY: `inputs` is a fully initialized slice of INPUT structures,
    // which SendInput copies and does not retain.
    let sent = unsafe { SendInput(&inputs, input_size) };
    if sent as usize != inputs.len() {
        return Err(io::Error::other(format!(
            "SendInput injected {sent} of {} events",
            inputs.len()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny layout of the test's own: lowercase letters and the space on
    /// their own keys, uppercase letters with Shift, and the euro sign with
    /// Control and Alt, as Alt Gr types it on many European layouts.
    fn layout(unit: u16) -> i16 {
        let Some(character) = char::from_u32(u32::from(unit)) else {
            return -1;
        };
        let upper = u16::try_from(u32::from(character.to_ascii_uppercase())).unwrap_or(0);
        let code = match character {
            'a'..='z' => upper,
            'A'..='Z' => 0x100 | upper,
            ' ' => 0x20,
            '€' => 0x600 | 0x45,
            '¤' => 0x800 | 0x45,
            _ => return -1,
        };
        i16::try_from(code).unwrap_or(-1)
    }

    fn down(key: VIRTUAL_KEY) -> Stroke {
        Stroke { key, up: false }
    }

    fn up(key: VIRTUAL_KEY) -> Stroke {
        Stroke { key, up: true }
    }

    #[test]
    fn plain_characters_press_and_release_their_keys() {
        let strokes = strokes_for("a b", layout).expect("types");
        assert_eq!(
            strokes,
            [
                down(VIRTUAL_KEY(0x41)),
                up(VIRTUAL_KEY(0x41)),
                down(VIRTUAL_KEY(0x20)),
                up(VIRTUAL_KEY(0x20)),
                down(VIRTUAL_KEY(0x42)),
                up(VIRTUAL_KEY(0x42)),
            ]
        );
    }

    #[test]
    fn a_shifted_character_holds_shift_around_its_key() {
        let strokes = strokes_for("E", layout).expect("types");
        assert_eq!(
            strokes,
            [
                down(VK_LSHIFT),
                down(VIRTUAL_KEY(0x45)),
                up(VIRTUAL_KEY(0x45)),
                up(VK_LSHIFT),
            ]
        );
    }

    #[test]
    fn altgr_holds_control_and_alt_and_releases_them_in_reverse() {
        let strokes = strokes_for("€", layout).expect("types");
        assert_eq!(
            strokes,
            [
                down(VK_LCONTROL),
                down(VK_LMENU),
                down(VIRTUAL_KEY(0x45)),
                up(VIRTUAL_KEY(0x45)),
                up(VK_LMENU),
                up(VK_LCONTROL),
            ]
        );
    }

    #[test]
    fn text_with_one_untypable_character_types_nothing() {
        let error = strokes_for("ab#c", layout).expect_err("# has no key");
        assert!(error.contains("'#'"), "names the character: {error}");
    }

    #[test]
    fn control_characters_and_unusual_shift_states_are_refused() {
        assert!(strokes_for("a\nb", layout).is_err());
        assert!(strokes_for("\t", layout).is_err());
        assert!(strokes_for("¤", layout).is_err());
        assert!(strokes_for("😀", layout).is_err());
    }

    #[test]
    fn empty_text_types_nothing() {
        assert_eq!(strokes_for("", layout), Ok(Vec::new()));
    }
}
