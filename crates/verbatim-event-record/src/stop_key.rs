//! The stop key, registered with `RegisterHotKey` so the recorder adds no
//! keyboard hook and no work per key.

use verbatim_control::send_keys::parse_combo;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey,
};

/// The default stop key: `RegisterHotKey` accepted it in the build's trial.
pub(crate) const DEFAULT_STOP_KEY: &str = "control+alt+shift+f12";

/// The hot key's id on the main thread.
pub(crate) const STOP_KEY_ID: i32 = 1;

/// A parsed stop key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StopKey {
    modifiers: HOT_KEY_MODIFIERS,
    vk: u32,
}

/// `MOD_*` for the modifier with virtual key `vk`, `None` for one
/// `RegisterHotKey` has no flag for.
fn modifier_flag(vk: u16) -> Option<HOT_KEY_MODIFIERS> {
    match vk {
        0x10 | 0xA0 | 0xA1 => Some(MOD_SHIFT),
        0x11 | 0xA2 | 0xA3 => Some(MOD_CONTROL),
        0x12 | 0xA4 | 0xA5 => Some(MOD_ALT),
        0x5B | 0x5C => Some(MOD_WIN),
        _ => None,
    }
}

/// Parses `keys` in the agent's key vocabulary.
///
/// # Errors
///
/// Returns a message for an unknown name or a modifier `RegisterHotKey`
/// has no flag for, naming `--stop-key`.
pub(crate) fn parse(keys: &str) -> Result<StopKey, String> {
    let combo = parse_combo(keys).map_err(|error| format!("--stop-key {keys}: {error}"))?;
    let mut modifiers = MOD_NOREPEAT;
    for modifier in &combo.modifiers {
        let flag = modifier_flag(modifier.vk).ok_or_else(|| {
            format!(
                "--stop-key {keys}: RegisterHotKey has no flag for modifier key {:#X}",
                modifier.vk
            )
        })?;
        modifiers |= flag;
    }
    Ok(StopKey {
        modifiers,
        vk: u32::from(combo.key.vk),
    })
}

/// Registers `key` on the calling thread.
///
/// # Errors
///
/// Returns a message naming `--stop-key` when Windows refuses it, as for a
/// key another program has registered.
pub(crate) fn register(key: StopKey, spelled: &str) -> Result<(), String> {
    // SAFETY: registers the key for the calling thread's own message
    // queue, with no window.
    unsafe { RegisterHotKey(None, STOP_KEY_ID, key.modifiers, key.vk) }
        .map_err(|error| format!("--stop-key {spelled}: Windows refused the key: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_stop_key_has_three_modifiers_and_f12() {
        assert_eq!(
            parse(DEFAULT_STOP_KEY),
            Ok(StopKey {
                modifiers: MOD_NOREPEAT | MOD_CONTROL | MOD_ALT | MOD_SHIFT,
                vk: 0x7B,
            })
        );
    }

    #[test]
    fn a_modifier_with_no_flag_is_refused() {
        assert_eq!(
            parse("capslock+f12"),
            Err(
                "--stop-key capslock+f12: RegisterHotKey has no flag for modifier key 0x14"
                    .to_owned()
            )
        );
    }
}
