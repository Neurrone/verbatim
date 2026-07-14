//! Parsing and synthesis for [`crate::protocol::Request::SendKeys`].
//!
//! Parsing is pure (no I/O, no injection) so it can be exhaustively unit
//! tested; [`parse_all`] validates every entry in a batch before
//! [`inject`] is ever called, so a batch containing one unknown key name
//! injects nothing at all.

use std::io;

use verbatim_input::keys::{KeyName, vk_from_name};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY,
};

/// Modifier names recognized in the all-but-last position of a plus-joined
/// `SendKeys` entry such as `shift+tab`. These are exactly the names
/// [`verbatim_input::keys::vk_from_name`] also resolves, so parsing only
/// needs to check membership before resolving the virtual key.
const MODIFIER_NAMES: &[&str] = &["control", "shift", "alt", "leftwindows", "rightwindows"];

/// One parsed `SendKeys` entry: zero or more modifiers held down for the
/// duration of one key press.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedCombo {
    /// Modifiers, in the order they were written; pressed down in this
    /// order and released in reverse.
    pub modifiers: Vec<KeyName>,
    /// The key pressed while the modifiers are held.
    pub key: KeyName,
}

/// Parses one plus-joined entry such as `shift+tab` or a bare `enter`.
///
/// # Errors
///
/// Returns a human-readable message when a modifier position holds a name
/// outside [`MODIFIER_NAMES`], or when any name fails to resolve via
/// [`vk_from_name`].
pub fn parse_combo(entry: &str) -> Result<ParsedCombo, String> {
    let parts: Vec<&str> = entry.split('+').collect();
    let Some((key_name, modifier_names)) = parts.split_last() else {
        return Err(format!("empty key combination: {entry:?}"));
    };
    if key_name.is_empty() {
        return Err(format!("empty key name in combination: {entry:?}"));
    }
    let mut modifiers = Vec::with_capacity(modifier_names.len());
    for name in modifier_names {
        if !MODIFIER_NAMES.contains(name) {
            return Err(format!("not a recognized modifier name: {name:?}"));
        }
        let resolved =
            vk_from_name(name).ok_or_else(|| format!("unknown modifier name: {name:?}"))?;
        modifiers.push(resolved);
    }
    let key = vk_from_name(key_name).ok_or_else(|| format!("unknown key name: {key_name:?}"))?;
    Ok(ParsedCombo { modifiers, key })
}

/// Parses every entry, gated: any unknown name in any entry fails the whole
/// batch before [`inject`] is ever called, so a bad entry injects nothing.
///
/// # Errors
///
/// Returns the first parse error encountered, in entry order.
pub fn parse_all(keys: &[String]) -> Result<Vec<ParsedCombo>, String> {
    keys.iter().map(|entry| parse_combo(entry)).collect()
}

/// Builds one `KEYBDINPUT` [`INPUT`] for a virtual key transition.
fn keybd_input(key: KeyName, key_up: bool) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if key.extended == Some(true) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if key_up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(key.vk),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Synthesizes real OS keyboard input for every parsed combo, in order:
/// modifier-downs, key-down, key-up, modifier-ups (reverse order).
///
/// # Errors
///
/// Returns an I/O error if `SendInput` reports it injected fewer events
/// than requested (the OS blocked injection, e.g. a secure desktop is
/// active).
///
/// # Safety-adjacent notes
///
/// This calls the Win32 `SendInput` API, which affects whatever
/// application currently has keyboard focus system-wide; callers must have
/// already validated every key name via [`parse_all`] before calling this,
/// per the crate's "reject before injecting anything" contract.
pub fn inject(combos: &[ParsedCombo]) -> io::Result<()> {
    let mut inputs = Vec::new();
    for combo in combos {
        for modifier in &combo.modifiers {
            inputs.push(keybd_input(*modifier, false));
        }
        inputs.push(keybd_input(combo.key, false));
        inputs.push(keybd_input(combo.key, true));
        for modifier in combo.modifiers.iter().rev() {
            inputs.push(keybd_input(*modifier, true));
        }
    }
    if inputs.is_empty() {
        return Ok(());
    }
    let input_size =
        i32::try_from(std::mem::size_of::<INPUT>()).expect("INPUT's size fits comfortably in i32");
    // Safety: `inputs` is a valid, live slice of properly initialized
    // `INPUT` values for the duration of this call; `SendInput` does not
    // retain the pointer afterward.
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

    #[test]
    fn bare_key_has_no_modifiers() {
        let combo = parse_combo("enter").expect("parses");
        assert!(combo.modifiers.is_empty());
        assert_eq!(combo.key, vk_from_name("enter").unwrap());
    }

    #[test]
    fn single_modifier_splits_correctly() {
        let combo = parse_combo("shift+tab").expect("parses");
        assert_eq!(combo.modifiers, vec![vk_from_name("shift").unwrap()]);
        assert_eq!(combo.key, vk_from_name("tab").unwrap());
    }

    #[test]
    fn multiple_modifiers_preserve_order() {
        let combo = parse_combo("control+alt+delete").expect("parses");
        assert_eq!(
            combo.modifiers,
            vec![
                vk_from_name("control").unwrap(),
                vk_from_name("alt").unwrap()
            ]
        );
        assert_eq!(combo.key, vk_from_name("delete").unwrap());
    }

    #[test]
    fn unknown_key_name_is_rejected() {
        assert!(parse_combo("hyperspace").is_err());
        assert!(parse_combo("shift+hyperspace").is_err());
    }

    #[test]
    fn unknown_modifier_name_is_rejected() {
        // "tab" is a real key name but never a valid modifier position.
        assert!(parse_combo("tab+enter").is_err());
    }

    #[test]
    fn empty_entry_is_rejected() {
        assert!(parse_combo("").is_err());
        assert!(parse_combo("shift+").is_err());
    }

    #[test]
    fn parse_all_rejects_whole_batch_on_one_bad_entry() {
        let keys = vec!["enter".to_owned(), "hyperspace".to_owned()];
        assert!(parse_all(&keys).is_err());
    }

    #[test]
    fn parse_all_resolves_every_entry_in_order() {
        let keys = vec!["downarrow".to_owned(), "shift+tab".to_owned()];
        let combos = parse_all(&keys).expect("parses");
        assert_eq!(combos.len(), 2);
        assert!(combos[0].modifiers.is_empty());
        assert_eq!(combos[1].modifiers.len(), 1);
    }
}
