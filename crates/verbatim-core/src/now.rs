//! The one time of the input Core is handling (`Now`), taken once in
//! [`crate::reduce`] from the input's arrival on the run's shared clock and
//! passed by value to whatever needs it while the input is handled: no
//! record of Core's holds a copy of it, so no reader judges by a time an
//! earlier input left behind.

use serde::{Deserialize, Serialize};

/// The time of the input being handled, in microseconds on the run's
/// shared clock (`verbatim_process::clock`). It has no default: there is
/// no time outside an input's handling. The front entry's checkpoint keeps
/// the time its entry started at, the one kept time, so that its function
/// rebuilt from a dump polls at the time it first ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct Now(u64);

impl Now {
    /// The time `arrival` came, which every effect and stamp of its
    /// handling uses.
    pub(crate) fn of(arrival: &verbatim_model::Arrival) -> Self {
        Self(arrival.at_us)
    }

    /// In microseconds.
    pub(crate) fn us(self) -> u64 {
        self.0
    }

    /// In milliseconds.
    pub(crate) fn ms(self) -> u64 {
        self.0 / 1_000
    }

    /// `us` microseconds from now.
    pub(crate) fn after(self, us: u64) -> u64 {
        self.0.saturating_add(us)
    }
}

#[cfg(test)]
impl Now {
    /// The time `us`, as a test's input carries it.
    pub(crate) const fn from_raw(us: u64) -> Self {
        Self(us)
    }
}
