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
///
/// The bare numpad digits (`numpad0` through `numpad9`) are the non-extended
/// twins of the navigation cluster, exactly like `insert`/`numpadinsert` and
/// `delete`/`numpaddelete` above: with Num Lock off, the physical numpad key
/// reports the same virtual-key code as its navigation-cluster counterpart,
/// distinguished only by the extended flag being clear. `numpad5` has no
/// navigation-cluster counterpart (that physical key does nothing there) and
/// reports `VK_CLEAR` instead, so it carries no twin to disambiguate against
/// and needs no required extended flag. The four numpad operator keys
/// (`numpadminus`, `numpadplus`, `numpaddivide`, `numpadmultiply`) and
/// `period` are ordinary single-vk keys with no twin, following the same
/// `None` convention as `escape` or `tab` above.
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
    ("numpad9", 0x21, Some(false)),
    ("pagedown", 0x22, Some(true)),
    ("numpad3", 0x22, Some(false)),
    ("end", 0x23, Some(true)),
    ("numpad1", 0x23, Some(false)),
    ("home", 0x24, Some(true)),
    ("numpad7", 0x24, Some(false)),
    ("leftarrow", 0x25, Some(true)),
    ("numpad4", 0x25, Some(false)),
    ("uparrow", 0x26, Some(true)),
    ("numpad8", 0x26, Some(false)),
    ("rightarrow", 0x27, Some(true)),
    ("numpad6", 0x27, Some(false)),
    ("downarrow", 0x28, Some(true)),
    ("numpad2", 0x28, Some(false)),
    ("numpad5", 0x0C, None),
    ("printscreen", 0x2C, None),
    ("insert", 0x2D, Some(true)),
    ("numpadinsert", 0x2D, Some(false)),
    ("numpad0", 0x2D, Some(false)),
    ("delete", 0x2E, Some(true)),
    ("numpaddelete", 0x2E, Some(false)),
    ("leftwindows", 0x5B, None),
    ("rightwindows", 0x5C, None),
    ("applications", 0x5D, None),
    ("numpadmultiply", 0x6A, None),
    ("numpadplus", 0x6B, None),
    ("numpadminus", 0x6D, None),
    ("numpaddivide", 0x6F, None),
    ("period", 0xBE, None),
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
    fn numpad_digits_share_vks_with_navigation_cluster_twins() {
        // With Num Lock off, the physical numpad digit reports the same vk as
        // its navigation-cluster counterpart, non-extended — the M3 desktop
        // layout's review-cursor keys.
        let cases = [
            ("numpad7", "home", 0x24),
            ("numpad8", "uparrow", 0x26),
            ("numpad9", "pageup", 0x21),
            ("numpad4", "leftarrow", 0x25),
            ("numpad6", "rightarrow", 0x27),
            ("numpad1", "end", 0x23),
            ("numpad2", "downarrow", 0x28),
            ("numpad3", "pagedown", 0x22),
            ("numpad0", "insert", 0x2D),
        ];
        for (numpad_name, nav_name, vk) in cases {
            assert_eq!(
                vk_from_name(numpad_name),
                Some(KeyName {
                    vk,
                    extended: Some(false)
                }),
                "{numpad_name} should be the non-extended twin of {nav_name}"
            );
            assert_eq!(
                vk_from_name(nav_name),
                Some(KeyName {
                    vk,
                    extended: Some(true)
                })
            );
        }
    }

    #[test]
    fn numpad5_has_no_navigation_cluster_twin() {
        // Numpad 5 with Num Lock off reports VK_CLEAR, which has no
        // navigation-cluster counterpart, so it needs no required extended
        // flag.
        assert_eq!(
            vk_from_name("numpad5"),
            Some(KeyName {
                vk: 0x0C,
                extended: None
            })
        );
        assert_eq!(name_from_vk(0x0C, false), Some("numpad5"));
    }

    #[test]
    fn numpad_operator_keys_resolve() {
        assert_eq!(
            vk_from_name("numpadminus"),
            Some(KeyName {
                vk: 0x6D,
                extended: None
            })
        );
        assert_eq!(
            vk_from_name("numpadplus"),
            Some(KeyName {
                vk: 0x6B,
                extended: None
            })
        );
        assert_eq!(
            vk_from_name("numpaddivide"),
            Some(KeyName {
                vk: 0x6F,
                extended: None
            })
        );
        assert_eq!(
            vk_from_name("numpadmultiply"),
            Some(KeyName {
                vk: 0x6A,
                extended: None
            })
        );
    }

    #[test]
    fn period_resolves() {
        assert_eq!(
            vk_from_name("period"),
            Some(KeyName {
                vk: 0xBE,
                extended: None
            })
        );
        assert_eq!(name_from_vk(0xBE, false), Some("period"));
    }

    #[test]
    fn numpad_enter_and_delete_already_present() {
        // Pinned so a future refactor cannot silently drop these M3-required
        // names; both existed before M3 and are re-checked here alongside
        // the new additions.
        assert_eq!(
            vk_from_name("numpadenter"),
            Some(KeyName {
                vk: 0x0D,
                extended: Some(true)
            })
        );
        assert_eq!(
            vk_from_name("numpaddelete"),
            Some(KeyName {
                vk: 0x2E,
                extended: Some(false)
            })
        );
    }

    #[test]
    fn unknown_names_are_rejected() {
        assert_eq!(vk_from_name("hyperspace"), None);
        assert_eq!(vk_from_name("A"), None, "names are lowercase only");
    }
}
