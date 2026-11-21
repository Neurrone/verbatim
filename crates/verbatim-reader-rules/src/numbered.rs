//! A reader's numbering of the messages it writes to Core that carry what
//! it keeps for Core: Core counts the same messages as it handles them, and
//! its held view acknowledges a position, so a reader lets go of a kept
//! thing only once Core has handled every message that carried it and does
//! not hold it. Both readers number this way, the listener its told
//! elements' tokens and an outpost its nodes, anchors and caret tells'
//! keys, so the rule is written once.

use std::collections::BTreeMap;

/// Where a kept thing was carried: the position of the last message
/// written that carried it, and how many messages carrying it still wait
/// to be written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The position of the last message written that carried it.
    pub position: u64,
    /// How many messages carrying it wait to be written.
    pub waiting: u32,
}

/// The writer's numbering of the messages that count, in the order they
/// are written, and the [`Report`] of each key they carried. Kept in the
/// writer's backlog, under its lock.
#[derive(Debug)]
pub struct Numbered<K> {
    /// How many messages that count have been written: the position of
    /// the last one.
    position: u64,
    reports: BTreeMap<K, Report>,
}

impl<K> Default for Numbered<K> {
    fn default() -> Self {
        Self {
            position: 0,
            reports: BTreeMap::new(),
        }
    }
}

impl<K: Ord + Copy> Numbered<K> {
    /// A message carrying `key` was queued to be written.
    pub fn queued(&mut self, key: K) {
        let report = self.reports.entry(key).or_default();
        report.waiting = report.waiting.saturating_add(1);
    }

    /// A message carrying `keys` is being written now: numbered when it
    /// `counts`, and each key's report records the position reached,
    /// unless the key was forgotten meanwhile ([`Self::forget`]).
    pub fn written(&mut self, counts: bool, keys: impl IntoIterator<Item = K>) {
        if counts {
            self.position = self.position.saturating_add(1);
        }
        for key in keys {
            if let Some(report) = self.reports.get_mut(&key) {
                report.waiting = report.waiting.saturating_sub(1);
                report.position = self.position;
            }
        }
    }

    /// A message carrying `key` that waited was replaced unwritten.
    pub fn withdrawn(&mut self, key: K) {
        if let Some(report) = self.reports.get_mut(&key) {
            report.waiting = report.waiting.saturating_sub(1);
        }
    }

    /// The report of `key`, `None` when no message carried it.
    #[must_use]
    pub fn report(&self, key: K) -> Option<Report> {
        self.reports.get(&key).copied()
    }

    /// Forgets `key`, which the reader let go of.
    pub fn forget(&mut self, key: K) {
        self.reports.remove(&key);
    }

    /// Forgets every key `gone` answers, which the reader let go of.
    pub fn forget_where(&mut self, gone: impl Fn(K) -> bool) {
        self.reports.retain(|&key, _| !gone(key));
    }

    /// The position of the last message that counts written.
    #[must_use]
    pub fn position(&self) -> u64 {
        self.position
    }
}

/// Whether a key whose report is `report` is too early for a view
/// acknowledging `acknowledged` to judge: no message carried it yet, one
/// still waits to be written, or the last was written after the position
/// Core acknowledges.
#[must_use]
pub fn too_early(report: Option<Report>, acknowledged: u64) -> bool {
    report.is_none_or(|report| report.waiting > 0 || report.position > acknowledged)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Too early: a key no message carried, one a message still waits to
    /// carry, and one written after the view's position; a key carried at
    /// or before it is not.
    #[test]
    fn a_key_not_yet_handled_is_too_early_to_judge() {
        let report = |position, waiting| Some(Report { position, waiting });
        assert!(too_early(None, 9));
        assert!(too_early(report(3, 1), 9));
        assert!(too_early(report(10, 0), 9));
        assert!(!too_early(report(9, 0), 9));
    }

    /// A message that counts moves the position on whatever it carries;
    /// one that does not records its keys at the position reached; a
    /// withdrawn message leaves its key's position, and a forgotten key
    /// has no report.
    #[test]
    fn the_messages_that_count_are_numbered_as_they_are_written() {
        let mut numbered = Numbered::default();
        numbered.queued(1_u64);
        numbered.queued(2);
        numbered.written(true, [1]);
        assert_eq!(
            numbered.report(2),
            Some(Report {
                position: 0,
                waiting: 1
            })
        );
        numbered.written(false, [2]);
        assert_eq!(
            numbered.report(2),
            Some(Report {
                position: 1,
                waiting: 0
            })
        );
        numbered.queued(1);
        numbered.withdrawn(1);
        assert_eq!(
            numbered.report(1),
            Some(Report {
                position: 1,
                waiting: 0
            })
        );
        numbered.written(true, []);
        assert_eq!(numbered.position(), 2);
        numbered.forget_where(|key| key == 2);
        numbered.forget(1);
        assert_eq!((numbered.report(1), numbered.report(2)), (None, None));
    }
}
