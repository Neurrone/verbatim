//! NVDA's limit on the events of one thread (`MAX_WINEVENTS_PER_THREAD`,
//! `nvda/source/IAccessibleHandler/orderedWinEventLimiter.py` 10, 104 to
//! 110), which both readers apply: an outpost's batch as it is taken, and
//! the focus listener's waiting value changes as one arrives.

use std::collections::BTreeMap;

/// How many of a thread's events are kept.
pub const EVENTS_PER_THREAD: usize = 10;

/// Counts events per raising thread, walked from the newest.
#[derive(Debug, Default)]
pub struct NewestPerThread {
    counts: BTreeMap<u32, usize>,
}

impl NewestPerThread {
    /// Counts one more event of `thread`, and whether it is within the
    /// limit.
    pub fn within(&mut self, thread: u32) -> bool {
        let count = self.counts.entry(thread).or_default();
        *count = count.saturating_add(1);
        *count <= EVENTS_PER_THREAD
    }
}

#[cfg(test)]
mod tests {
    use super::{EVENTS_PER_THREAD, NewestPerThread};

    /// A thread's tenth is within the limit and its eleventh is not, and
    /// another thread counts apart.
    #[test]
    fn a_thread_keeps_its_newest_ten() {
        let mut counter = NewestPerThread::default();
        for _ in 0..EVENTS_PER_THREAD {
            assert!(counter.within(7));
        }
        assert!(!counter.within(7));
        assert!(counter.within(8));
    }
}
