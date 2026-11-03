//! The observation point of a producer's wait for room, compiled only with
//! the `faults` feature, which only `cargo xtask faults` turns on
//! (`docs/crates/verbatim-audio.md`): a test waits for a producer to have
//! begun waiting before it checks that the producer stays blocked. Only
//! producers reach it, never the audio thread.

use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Duration;

/// How many waits for room the producers on one source have begun.
#[derive(Default)]
pub(crate) struct RoomWaits {
    began: Mutex<u64>,
    changed: Condvar,
}

impl RoomWaits {
    /// Counts a wait a producer begins.
    pub(crate) fn begin(&self) {
        *self.began.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        self.changed.notify_all();
    }

    /// Waits up to `timeout` until at least `count` waits have begun;
    /// returns whether they have.
    pub(crate) fn wait_began(&self, count: u64, timeout: Duration) -> bool {
        let began = self.began.lock().unwrap_or_else(PoisonError::into_inner);
        let (began, _) = self
            .changed
            .wait_timeout_while(began, timeout, |began| *began < count)
            .unwrap_or_else(PoisonError::into_inner);
        *began >= count
    }
}
