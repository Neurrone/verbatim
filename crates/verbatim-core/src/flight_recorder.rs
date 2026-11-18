//! Always-on, bounded recording of recent reducer history (architecture
//! section 9).
//!
//! The recorder runs continuously on the reducer thread, so it must never
//! block, never touch disk, and never grow: it holds a bounded window of
//! recent entries in memory, bounded both by entry count and by an estimate
//! of the bytes the entries hold, dropping the oldest as new ones arrive.
//! Nothing is persisted until a crash handler or an explicit user gesture
//! takes a dump; the dump replays deterministically against the reducer,
//! turning field bugs into regression tests.
//!
//! A window of inputs replays only from the state it started in, because
//! the inputs that built that state have already been dropped. So the
//! recorder keeps, with its entries, a checkpoint: a snapshot of the state
//! taken just before the oldest entry it keeps.
//!
//! # The checkpoint rule
//!
//! The window is a list of segments, each starting with a checkpoint and
//! holding the entries recorded after it.
//!
//! - Each entry is appended to the newest segment.
//! - When the newest segment holds half the entry bound, or half the byte
//!   bound, a new segment is started with a checkpoint taken right after
//!   that entry.
//! - While the window holds more than either bound, its oldest segment is
//!   dropped whole, so the window always starts at a checkpoint.
//!
//! So the window always starts exactly at a checkpoint, normally holds
//! between half and all of each bound, and a checkpoint is taken once per
//! half window, never per entry. At most three checkpoints are alive at
//! once. Their own size is not counted against the byte bound: the reducer
//! state holds everything that grows behind shared pointers, so a
//! checkpoint costs the same however large the application is.
//!
//! # Dumping without holding up the recorder
//!
//! A dump runs on another thread while the reducer keeps recording, and
//! both share the recorder behind one lock. So nothing a dump needs is
//! copied under that lock: each segment's entries form a list of shared,
//! immutable links, each new entry pointing at the one before it, and its
//! checkpoint is shared too. [`FlightRecorder::snapshot`] takes a reference
//! to each segment's checkpoint and newest link, a step that grows with
//! the number of segments (at most three in a window that has dropped one)
//! and not with the entries; the dump reads and writes the entries after
//! the lock is released, while the recorder appends new links and drops
//! old segments without disturbing what the dump holds.

use std::collections::VecDeque;
use std::sync::Arc;

/// One entry and the link to the entry recorded before it in the same
/// segment. Links are never changed once made, so a snapshot can share
/// them with the recorder.
#[derive(Debug)]
struct Link<T> {
    entry: T,
    previous: Option<Arc<Link<T>>>,
}

impl<T> Drop for Link<T> {
    /// Frees the chain behind this link one link at a time, so dropping a
    /// long segment never recurses once per entry. A link someone else
    /// still holds stops the walk; its last holder frees the rest.
    fn drop(&mut self) {
        let mut next = self.previous.take();
        while let Some(link) = next {
            match Arc::try_unwrap(link) {
                Ok(mut owned) => next = owned.previous.take(),
                Err(_shared) => break,
            }
        }
    }
}

/// Appends the entries of the chain ending at `newest` to `out`, oldest
/// first.
fn push_chain<'a, T>(newest: Option<&'a Arc<Link<T>>>, out: &mut Vec<&'a T>) {
    let mut chain = Vec::new();
    let mut link = newest;
    while let Some(current) = link {
        chain.push(&current.entry);
        link = current.previous.as_ref();
    }
    out.extend(chain.into_iter().rev());
}

/// One run of entries recorded after a checkpoint.
#[derive(Debug)]
struct Segment<T, S> {
    /// The state just before this segment's first entry.
    checkpoint: Arc<S>,
    /// The newest entry, which links back to the oldest; `None` while the
    /// segment is empty.
    newest: Option<Arc<Link<T>>>,
    /// The number of entries.
    len: usize,
    /// The sum of the entries' estimated sizes.
    bytes: usize,
}

impl<T, S> Segment<T, S> {
    /// An empty segment starting at `checkpoint`.
    fn starting_at(checkpoint: S) -> Self {
        Self {
            checkpoint: Arc::new(checkpoint),
            newest: None,
            len: 0,
            bytes: 0,
        }
    }
}

/// The window as it stood when [`FlightRecorder::snapshot`] took it,
/// sharing its entries and checkpoint with the recorder, which goes on
/// recording.
#[derive(Debug)]
pub struct FlightSnapshot<T, S> {
    checkpoint: Arc<S>,
    /// Each segment's newest link, oldest segment first.
    segments: Vec<Option<Arc<Link<T>>>>,
    len: usize,
}

impl<T, S> FlightSnapshot<T, S> {
    /// The state just before the oldest entry: where a replay of
    /// [`Self::entries`] starts.
    #[must_use]
    pub fn checkpoint(&self) -> &S {
        &self.checkpoint
    }

    /// The entries, oldest first.
    #[must_use]
    pub fn entries(&self) -> Vec<&T> {
        let mut out = Vec::with_capacity(self.len);
        for newest in &self.segments {
            push_chain(newest.as_ref(), &mut out);
        }
        out
    }

    /// Number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// True when the snapshot holds no entry.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// A bounded window of recent entries of type `T`, replayable from the
/// checkpoint of type `S` it starts at (see the module documentation for
/// the checkpoint rule).
#[derive(Debug)]
pub struct FlightRecorder<T, S> {
    /// The oldest segment, whose checkpoint is the window's start.
    oldest: Segment<T, S>,
    /// The segments after it, oldest first; the last is the one entries
    /// are appended to, or `oldest` while this is empty.
    newer: VecDeque<Segment<T, S>>,
    max_entries: usize,
    max_bytes: usize,
    entries: usize,
    bytes: usize,
}

impl<T, S> FlightRecorder<T, S> {
    /// Creates a recorder keeping at most `max_entries` entries and at most
    /// `max_bytes` estimated bytes of entries, starting from `initial`, the
    /// state before anything is recorded.
    ///
    /// # Panics
    ///
    /// Panics if either bound is zero: a recorder that can hold nothing would
    /// silently discard every entry, which is never intended.
    #[must_use]
    pub fn new(max_entries: usize, max_bytes: usize, initial: S) -> Self {
        assert!(
            max_entries > 0 && max_bytes > 0,
            "flight recorder capacity must be non-zero"
        );
        Self {
            oldest: Segment::starting_at(initial),
            newer: VecDeque::with_capacity(2),
            max_entries,
            max_bytes,
            entries: 0,
            bytes: 0,
        }
    }

    /// Records one entry whose estimated size is `bytes`, dropping the
    /// oldest segments while the window is over either bound.
    /// `checkpoint` is called, at most once, when this entry closes its
    /// segment: it returns the state right after this entry, which starts
    /// the next segment.
    ///
    /// An entry larger than the whole byte bound is not kept, so the bound
    /// always holds.
    pub fn record(&mut self, entry: T, bytes: usize, checkpoint: impl FnOnce() -> S) {
        let newest = self.newer.back_mut().unwrap_or(&mut self.oldest);
        newest.newest = Some(Arc::new(Link {
            entry,
            previous: newest.newest.take(),
        }));
        newest.len = newest.len.saturating_add(1);
        newest.bytes = newest.bytes.saturating_add(bytes);
        self.entries = self.entries.saturating_add(1);
        self.bytes = self.bytes.saturating_add(bytes);
        if newest.len >= self.max_entries.div_ceil(2) || newest.bytes >= self.max_bytes.div_ceil(2)
        {
            self.newer.push_back(Segment::starting_at(checkpoint()));
        }
        while self.entries > self.max_entries || self.bytes > self.max_bytes {
            let Some(next) = self.newer.pop_front() else {
                break;
            };
            let dropped = std::mem::replace(&mut self.oldest, next);
            self.entries = self.entries.saturating_sub(dropped.len);
            self.bytes = self.bytes.saturating_sub(dropped.bytes);
        }
    }

    /// The window as it stands, sharing its entries and checkpoint rather
    /// than copying them: a reference to each segment's checkpoint and
    /// newest link, so the step grows with the number of segments (at most
    /// three once the window has dropped one) and never with the entries.
    #[must_use]
    pub fn snapshot(&self) -> FlightSnapshot<T, S> {
        FlightSnapshot {
            checkpoint: Arc::clone(&self.oldest.checkpoint),
            segments: std::iter::once(&self.oldest)
                .chain(&self.newer)
                .map(|segment| segment.newest.clone())
                .collect(),
            len: self.entries,
        }
    }

    /// The state just before the oldest kept entry: where a replay of
    /// [`Self::entries`] starts.
    #[must_use]
    pub fn checkpoint(&self) -> &S {
        &self.oldest.checkpoint
    }

    /// The kept entries, oldest first.
    #[must_use]
    pub fn entries(&self) -> Vec<&T> {
        let mut out = Vec::with_capacity(self.entries);
        for segment in std::iter::once(&self.oldest).chain(&self.newer) {
            push_chain(segment.newest.as_ref(), &mut out);
        }
        out
    }

    /// Number of entries currently kept.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries
    }

    /// True while no entry is kept.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries == 0
    }

    /// The estimated bytes of the entries currently kept.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Maximum number of entries the recorder keeps.
    #[must_use]
    pub fn max_entries(&self) -> usize {
        self.max_entries
    }

    /// Maximum estimated bytes of entries the recorder keeps.
    #[must_use]
    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A recorder of numbers whose checkpoint is the last number recorded
    /// before the window, so a test can check that the window starts at
    /// its checkpoint.
    fn record_all(recorder: &mut FlightRecorder<i32, i32>, values: impl Iterator<Item = i32>) {
        for value in values {
            recorder.record(value, 1, || value);
        }
    }

    fn kept(recorder: &FlightRecorder<i32, i32>) -> Vec<i32> {
        recorder.entries().into_iter().copied().collect()
    }

    #[test]
    fn retains_everything_below_capacity() {
        let mut recorder = FlightRecorder::new(4, 100, 0);
        record_all(&mut recorder, 1..=2);
        assert_eq!(recorder.len(), 2);
        assert_eq!(kept(&recorder), [1, 2]);
        assert_eq!(*recorder.checkpoint(), 0);
    }

    #[test]
    fn drops_oldest_first_once_full_and_starts_at_a_checkpoint() {
        let mut recorder = FlightRecorder::new(4, 100, 0);
        record_all(&mut recorder, 1..=5);
        assert_eq!(kept(&recorder), [3, 4, 5]);
        assert_eq!(*recorder.checkpoint(), 2, "the state just before 3");
    }

    #[test]
    fn the_window_always_starts_at_its_checkpoint() {
        // With room for six, a segment closes every three entries, and the
        // oldest segment goes when a seventh entry arrives. So after
        // entry `last` the window starts at the first entry of the segment
        // before the one `last` belongs to: entries 1 to 6 are all kept,
        // then 4 to 7, 4 to 8 and 4 to 9, then 7 to 10, and so on.
        let mut recorder = FlightRecorder::new(6, 100, 0);
        for last in 1..=50 {
            record_all(&mut recorder, last..=last);
            let first = (3 * ((last - 1) / 3) - 2).max(1);
            assert_eq!(kept(&recorder), (first..=last).collect::<Vec<_>>());
            assert_eq!(*recorder.checkpoint(), first - 1);
        }
    }

    #[test]
    fn memory_use_is_bounded_by_capacity() {
        // With room for two, every entry closes its segment, so the window
        // holds the last two entries, and after them an empty segment
        // waits for the next.
        let mut recorder = FlightRecorder::new(2, 100, 0);
        record_all(&mut recorder, 0..1000);
        assert_eq!(kept(&recorder), [998, 999]);
        assert_eq!(recorder.len(), 2);
        assert_eq!(*recorder.checkpoint(), 997);
        assert_eq!(recorder.newer.len(), 2);
    }

    #[test]
    fn large_entries_are_bounded_by_bytes() {
        // Entry `index` holds 3,000 + `index` bytes, so two entries pass
        // half the 10,000-byte bound and close a segment, and a fourth
        // entry in the window passes the bound and drops the oldest two.
        // After an even entry the window holds the last three; after an
        // odd one, the last two.
        let mut recorder = FlightRecorder::new(1000, 10_000, String::new());
        for index in 0..100 {
            let text = "x".repeat(3_000 + index);
            let bytes = text.len();
            recorder.record(text, bytes, String::new);
            let first = match index {
                0 => 0,
                odd if odd % 2 == 1 => odd - 1,
                even => even - 2,
            };
            let expected: Vec<usize> = (first..=index).map(|kept| 3_000 + kept).collect();
            let actual: Vec<usize> = recorder.entries().into_iter().map(String::len).collect();
            assert_eq!(actual, expected, "after entry {index}");
            assert_eq!(recorder.bytes(), expected.iter().sum::<usize>());
        }
    }

    #[test]
    fn an_entry_larger_than_the_byte_bound_is_not_kept() {
        let mut recorder = FlightRecorder::new(10, 100, 0);
        recorder.record(1, 10, || 1);
        recorder.record(2, 1_000, || 2);
        assert_eq!(recorder.bytes(), 0);
        assert_eq!(*recorder.checkpoint(), 2);
        assert!(recorder.is_empty());
    }

    #[test]
    fn a_snapshot_is_unchanged_by_later_recording_and_leaves_the_window() {
        let mut recorder = FlightRecorder::new(4, 100, 0);
        record_all(&mut recorder, 1..=5);
        let snapshot = recorder.snapshot();
        assert_eq!(kept(&recorder), [3, 4, 5], "taking it keeps the window");

        record_all(&mut recorder, 6..=20);
        assert_eq!(snapshot.entries(), [&3, &4, &5]);
        assert_eq!(snapshot.len(), 3);
        assert_eq!(*snapshot.checkpoint(), 2);
        assert_eq!(kept(&recorder), [17, 18, 19, 20]);
        assert_eq!(*recorder.checkpoint(), 16);
    }

    /// An entry that cannot be cloned, so a snapshot can only share it.
    struct Unclonable(u32);

    /// The step a dump takes under the recorder's lock is
    /// [`FlightRecorder::snapshot`]: it asks nothing of the entries or the
    /// checkpoint (neither type is `Clone` here), and it holds one reference
    /// per segment, the very links the recorder holds, however many entries
    /// they carry. Dropping a 50,000-entry segment afterwards does not
    /// recurse once per entry.
    #[test]
    fn a_snapshot_shares_each_segment_whatever_its_length() {
        let mut recorder = FlightRecorder::new(100_000, usize::MAX, Unclonable(0));
        for value in 1..=200_000 {
            recorder.record(Unclonable(value), 1, || Unclonable(value));
        }
        let snapshot = recorder.snapshot();

        assert_eq!(snapshot.segments.len(), 3);
        let held = std::iter::once(&recorder.oldest).chain(&recorder.newer);
        for (shared, segment) in snapshot.segments.iter().zip(held) {
            match (shared, &segment.newest) {
                (Some(shared), Some(own)) => assert!(Arc::ptr_eq(shared, own)),
                (None, None) => {}
                _ => panic!("the snapshot and the recorder differ on a segment"),
            }
        }
        assert!(Arc::ptr_eq(
            &snapshot.checkpoint,
            &recorder.oldest.checkpoint
        ));
        let entries: Vec<u32> = snapshot.entries().iter().map(|entry| entry.0).collect();
        assert_eq!(entries, (100_001..=200_000).collect::<Vec<_>>());
        assert_eq!(snapshot.checkpoint().0, 100_000);

        drop(recorder);
        assert_eq!(snapshot.len(), 100_000);
        drop(snapshot);
    }

    #[test]
    #[should_panic(expected = "capacity must be non-zero")]
    fn zero_capacity_is_rejected() {
        let _ = FlightRecorder::<i32, ()>::new(0, 1, ());
    }
}
