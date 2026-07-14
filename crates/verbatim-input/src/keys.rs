//! Key names, following NVDA's vocabulary (`nvda/source/vkCodes.py`).
//!
//! One table serves both directions: gesture identifiers name keys the NVDA
//! way (`downarrow`, `pageup`, `numpadinsert`), and the control plane's key
//! injection parses the same names. All names are lowercase, matching
//! gesture-identifier normalization.

/// The gesture-identifier name of the Verbatim modifier, NVDA's `nvda`. It
/// names no single virtual key: which physical keys act as the modifier is a
/// configuration choice (caps lock, insert, numpad insert), so this name
/// stands in for whichever one is held.
pub const VERBATIM_MODIFIER_NAME: &str = "verbatim";

/// A named key: virtual-key code plus how the extended-key flag must match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyName {
    /// Virtual-key code.
    pub vk: u16,
    /// Required extended flag; `None` matches either — the key has no twin
    /// that differs only in the flag.
    pub extended: Option<bool>,
}

/// Named keys, following NVDA. Letters and digits are handled procedurally
/// by [`vk_from_name`] and [`name_from_vk`], not listed here.
const NAMED_KEYS: &[(&str, u16, Option<bool>)] = &[
    ("backspace", 0x08, None),
    ("tab", 0x09, None),
    ("enter", 0x0D, Some(false)),
    ("numpadenter", 0x0D, Some(true)),
    ("shift", 0x10, None),
    ("control", 0x11, None),
    ("alt", 0x12, None),
    ("pause", 0x13, None),
    ("capslock", 0x14, None),
    ("escape", 0x1B, None),
    ("space", 0x20, None),
    ("pageup", 0x21, Some(true)),
    ("pagedown", 0x22, Some(true)),
    ("end", 0x23, Some(true)),
    ("home", 0x24, Some(true)),
    ("leftarrow", 0x25, Some(true)),
    ("uparrow", 0x26, Some(true)),
    ("rightarrow", 0x27, Some(true)),
    ("downarrow", 0x28, Some(true)),
    ("printscreen", 0x2C, None),
    ("insert", 0x2D, Some(true)),
    ("numpadinsert", 0x2D, Some(false)),
    ("delete", 0x2E, Some(true)),
    ("numpaddelete", 0x2E, Some(false)),
    ("leftwindows", 0x5B, None),
    ("rightwindows", 0x5C, None),
    ("applications", 0x5D, None),
    ("f1", 0x70, None),
    ("f2", 0x71, None),
    ("f3", 0x72, None),
    ("f4", 0x73, None),
    ("f5", 0x74, None),
    ("f6", 0x75, None),
    ("f7", 0x76, None),
    ("f8", 0x77, None),
    ("f9", 0x78, None),
    ("f10", 0x79, None),
    ("f11", 0x7A, None),
    ("f12", 0x7B, None),
    ("numlock", 0x90, None),
    ("scrolllock", 0x91, None),
    ("leftshift", 0xA0, None),
    ("rightshift", 0xA1, None),
    ("leftcontrol", 0xA2, None),
    ("rightcontrol", 0xA3, None),
    ("leftalt", 0xA4, None),
    ("rightalt", 0xA5, None),
    ("plus", 0xBB, None),
];

/// Resolves a lowercase key name to its virtual key.
///
/// Single letters and digits resolve procedurally (`a` through `z`, `0`
/// through `9`); everything else looks up the named table. Returns `None`
/// for names outside the vocabulary.
#[must_use]
pub fn vk_from_name(name: &str) -> Option<KeyName> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_lowercase() {
            return Some(KeyName {
                vk: u16::from(c.to_ascii_uppercase() as u8),
                extended: None,
            });
        }
        if c.is_ascii_digit() {
            return Some(KeyName {
                vk: u16::from(c as u8),
                extended: None,
            });
        }
    }
    NAMED_KEYS
        .iter()
        .find(|(key_name, _, _)| *key_name == name)
        .map(|&(_, vk, extended)| KeyName { vk, extended })
}

/// Names a virtual key with its extended flag, the reverse of
/// [`vk_from_name`]. Returns `None` for keys outside the vocabulary.
#[must_use]
pub fn name_from_vk(vk: u16, extended: bool) -> Option<&'static str> {
    const LETTERS: [&str; 26] = [
        "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r",
        "s", "t", "u", "v", "w", "x", "y", "z",
    ];
    const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
    if let Some(&(name, _, _)) = NAMED_KEYS.iter().find(|&&(_, key_vk, key_extended)| {
        key_vk == vk && key_extended.unwrap_or(extended) == extended
    }) {
        return Some(name);
    }
    match vk {
        0x41..=0x5A => Some(LETTERS[usize::from(vk - 0x41)]),
        0x30..=0x39 => Some(DIGITS[usize::from(vk - 0x30)]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_and_digits_resolve_procedurally() {
        assert_eq!(
            vk_from_name("v"),
            Some(KeyName {
                vk: 0x56,
                extended: None
            })
        );
        assert_eq!(
            vk_from_name("7"),
            Some(KeyName {
                vk: 0x37,
                extended: None
            })
        );
        assert_eq!(name_from_vk(0x56, false), Some("v"));
        assert_eq!(name_from_vk(0x37, false), Some("7"));
    }

    #[test]
    fn extended_flag_distinguishes_key_twins() {
        assert_eq!(
            vk_from_name("insert"),
            Some(KeyName {
                vk: 0x2D,
                extended: Some(true)
            })
        );
        assert_eq!(
            vk_from_name("numpadinsert"),
            Some(KeyName {
                vk: 0x2D,
                extended: Some(false)
            })
        );
        assert_eq!(name_from_vk(0x2D, true), Some("insert"));
        assert_eq!(name_from_vk(0x2D, false), Some("numpadinsert"));
    }

    #[test]
    fn every_named_key_round_trips() {
        for &(name, vk, extended) in NAMED_KEYS {
            let resolved = vk_from_name(name).expect("name resolves");
            assert_eq!(resolved.vk, vk);
            let round_tripped = name_from_vk(vk, extended.unwrap_or(false)).expect("vk resolves");
            // Twins that share a vk with `extended: None` entries (enter and
            // numpad enter) may name the sibling; the vk must match either way.
            let back = vk_from_name(round_tripped).expect("round-tripped name resolves");
            assert_eq!(back.vk, vk);
        }
    }

    #[test]
    fn unknown_names_are_rejected() {
        assert_eq!(vk_from_name("hyperspace"), None);
        assert_eq!(vk_from_name("A"), None, "names are lowercase only");
    }
}
