//! The keys that toggle a lock: Caps Lock, Num Lock, and Scroll Lock.
//!
//! When one of them reaches the operating system (pressed on its own, or
//! Caps Lock passed through by a double tap of the Verbatim key), its new
//! state is announced ("caps lock on"), as NVDA announces it
//! (`docs/parity.md`, "Toggle key announcements").

use verbatim_model::GestureId;

/// A key that toggles a lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToggleKey {
    /// Caps Lock.
    CapsLock,
    /// Num Lock.
    NumLock,
    /// Scroll Lock.
    ScrollLock,
}

impl ToggleKey {
    /// The toggle key with virtual-key code `vk`, if it is one.
    #[must_use]
    pub fn from_vk(vk: u16) -> Option<Self> {
        match vk {
            0x14 => Some(Self::CapsLock),
            0x90 => Some(Self::NumLock),
            0x91 => Some(Self::ScrollLock),
            _ => None,
        }
    }

    /// Its virtual-key code.
    #[must_use]
    pub fn vk(self) -> u16 {
        match self {
            Self::CapsLock => 0x14,
            Self::NumLock => 0x90,
            Self::ScrollLock => 0x91,
        }
    }

    /// The gesture that reports it reached the operating system: its key
    /// alone, which is never bound, since a bound key would not reach the
    /// operating system.
    ///
    /// # Panics
    ///
    /// Never: the three identifiers are fixed and well-formed.
    #[must_use]
    pub fn gesture(self) -> GestureId {
        let name = match self {
            Self::CapsLock => "kb:capslock",
            Self::NumLock => "kb:numlock",
            Self::ScrollLock => "kb:scrolllock",
        };
        GestureId::parse(name).expect("a toggle key's gesture is well-formed")
    }

    /// The toggle key a gesture reports, if it reports one.
    #[must_use]
    pub fn of_gesture(gesture: &GestureId) -> Option<Self> {
        [Self::CapsLock, Self::NumLock, Self::ScrollLock]
            .into_iter()
            .find(|key| key.gesture() == *gesture)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_toggle_key_round_trips_through_its_code_and_gesture() {
        for key in [
            ToggleKey::CapsLock,
            ToggleKey::NumLock,
            ToggleKey::ScrollLock,
        ] {
            assert_eq!(ToggleKey::from_vk(key.vk()), Some(key));
            assert_eq!(ToggleKey::of_gesture(&key.gesture()), Some(key));
        }
        assert_eq!(ToggleKey::from_vk(0x41), None);
    }
}
