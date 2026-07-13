//! Always-on, bounded recording of recent reducer history (architecture
//! section 9).
//!
//! The recorder runs continuously on the reducer thread, so it must never
//! block, never touch disk, and never grow: it holds a fixed number of
//! recent entries in memory, overwriting the oldest as new ones arrive.
//! Nothing is persisted until a crash handler or an explicit user gesture
//! takes a snapshot; the resulting dump replays deterministically against
//! the pure reducer, turning field bugs into regression tests.
//!
//! M0 scope: the ring itself. Rotating state snapshots (so a dump always
//! carries a base state at least as old as its oldest input) and the dump
//! format arrive with the real reducer in M1.

use std::collections::VecDeque;

/// Fixed-capacity ring buffer over recent reducer inputs (and, later,
/// tracing spans).
#[derive(Debug)]
pub struct FlightRecorder<T> {
    entries: VecDeque<T>,
    capacity: usize,
}

impl<T> FlightRecorder<T> {
    /// Creates a recorder retaining at most `capacity` entries.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is zero: a recorder that can hold nothing would
    /// silently discard every entry, which is never intended.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "flight recorder capacity must be non-zero");
        Self {
            entries: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Records one entry, evicting the oldest when the buffer is full.
    pub fn record(&mut self, entry: T) {
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    /// The retained entries, oldest first.
    pub fn snapshot(&self) -> impl Iterator<Item = &T> {
        self.entries.iter()
    }

    /// Number of entries currently retained.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True while nothing has been recorded yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Maximum number of entries the recorder retains.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retains_everything_below_capacity() {
        let mut recorder = FlightRecorder::new(4);
        recorder.record(1);
        recorder.record(2);
        assert_eq!(recorder.len(), 2);
        assert_eq!(recorder.snapshot().copied().collect::<Vec<_>>(), [1, 2]);
    }

    #[test]
    fn evicts_oldest_first_once_full() {
        let mut recorder = FlightRecorder::new(3);
        for value in 1..=5 {
            recorder.record(value);
        }
        assert_eq!(recorder.len(), 3);
        assert_eq!(recorder.snapshot().copied().collect::<Vec<_>>(), [3, 4, 5]);
    }

    #[test]
    fn memory_use_is_bounded_by_capacity() {
        let mut recorder = FlightRecorder::new(2);
        for value in 0..1000 {
            recorder.record(value);
        }
        assert!(recorder.entries.capacity() < 1000);
        assert_eq!(recorder.snapshot().copied().collect::<Vec<_>>(), [998, 999]);
    }

    #[test]
    #[should_panic(expected = "capacity must be non-zero")]
    fn zero_capacity_is_rejected() {
        let _ = FlightRecorder::<i32>::new(0);
    }
}
