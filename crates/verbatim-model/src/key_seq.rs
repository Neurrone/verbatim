//! The key number: the order of the user's inputs, which fences speech
//! (`docs/design/ownership-input.md`, "One minter, on the hook thread").

use serde::{Deserialize, Serialize};

/// The number of a user input, ordered by press: a key press the keyboard
/// hook decides, or an input injected for the user, numbered by the hook
/// thread in the order it handles them. Speech a user input caused carries
/// its number, and a later input's cancel drops speech of an earlier one
/// that arrives after it.
///
/// Only the hook thread mints numbers ([`KeySeq::after`]); that is a review
/// rule, since this type lives in a platform-neutral crate. Tests construct
/// numbers freely ([`KeySeq::new`]).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct KeySeq(u64);

impl KeySeq {
    /// No number given out yet: the hook's start, and what an input from a
    /// flight dump made before inputs carried numbers is read with.
    pub const NONE_YET: Self = Self(0);

    /// The number `number`.
    #[must_use]
    pub const fn new(number: u64) -> Self {
        Self(number)
    }

    /// The number after `last`, the last given out.
    #[must_use]
    pub const fn after(last: Self) -> Self {
        Self(last.0.wrapping_add(1))
    }

    /// The number as an integer.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// [`KeySeq::NONE_YET`], the serde default of every key field, so an
    /// input from an old flight dump reads with no number.
    #[must_use]
    pub const fn none_yet() -> Self {
        Self::NONE_YET
    }
}

impl std::fmt::Display for KeySeq {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}
