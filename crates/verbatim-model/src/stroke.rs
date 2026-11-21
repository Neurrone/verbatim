//! Key strokes the keyboard hook held or deferred, as Core's stroke line
//! keeps them and names them to the shell to send
//! (`books/dev/src/reference/crates/verbatim-core.md`, "The caret key on the event queue"). The
//! shell keeps no line of its own: it sends
//! what Core names, with one `SendInput`, and at exit releases the keys
//! Core lists that Windows still has down.

use serde::{Deserialize, Serialize};

/// `VK_PACKET`: a character typed as Unicode, its UTF-16 unit in the scan
/// code.
pub const VK_PACKET: u16 = 0xE7;

/// The key a held or deferred transition came from, for the application
/// to be sent it again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stroke {
    /// The virtual-key code; [`VK_PACKET`] for a character typed as
    /// Unicode, whose UTF-16 unit is the scan code.
    pub vk: u16,
    /// The hardware scan code.
    pub scan_code: u32,
    /// Whether it is an extended key (the arrows and the navigation block).
    pub extended: bool,
    /// Its place in the hook's line, counted from 1, which the key carries
    /// when it is sent so the hook knows it has reached the application.
    pub sequence: u32,
}

impl Stroke {
    /// Whether it is a character typed as Unicode, which leaves no key
    /// down.
    #[must_use]
    pub const fn is_unicode(self) -> bool {
        self.vk == VK_PACKET
    }
}

/// A modifier of a held key's gesture, pressed around it when it is sent
/// and not already down.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Modifier {
    /// Shift.
    Shift,
    /// Control.
    Control,
    /// Alt.
    Alt,
}

/// A caret key the hook held back, with its gesture's modifiers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldStroke {
    /// The key.
    pub stroke: Stroke,
    /// Its gesture's modifiers, in the order Shift, Control, Alt.
    pub modifiers: Vec<Modifier>,
}

/// One transition Core names for the shell to send, in line order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SentTransition {
    /// A held key: sent as its gesture is, its modifiers not down pressed
    /// first and released after, its press and its release.
    Held(HeldStroke),
    /// A transition deferred behind a held key.
    Deferred {
        /// The key.
        stroke: Stroke,
        /// A press, or else a release.
        pressed: bool,
    },
}
