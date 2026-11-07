//! Core's live re-reads (`books/dev/src/reference/crates/verbatim-core.md`,
//! "The ancestry cache"): a focus report that would speak an ancestor its
//! focus reader took from its ancestry cache, as entered or as the title,
//! is set aside, and the focus reader that sent it is asked to read the
//! same element again live, with no ancestry cache. Its answer carries no
//! cached ancestors, so it never needs another.
//!
//! One live re-read is pending per focus reader, an outpost generation
//! with a kind (the listener's reading thread or the outpost's own reads).
//! A pending live re-read ends on exactly one path: its answer, applied as
//! an ordinary report; its answer of gone; a newer focus applied; a newer
//! live re-read of the same focus reader, which replaces it; its outpost
//! generation's end; or, for the listener's, the listener's replacement.
//! An answer to a live re-read that is no longer pending is ignored, so a
//! late answer is set aside by Core's own record, whichever ending came
//! first.
//!
//! A repeat of a pending live re-read's element is no duplicate of the
//! report set aside: it is judged as any new report, so it either asks a
//! newer live re-read under its own trace, replacing the pending one, or is
//! taken, ending it. The reader answers a live re-read only for the read
//! it holds last, which the repeat replaced, so the older request would be
//! answered gone and the focus never spoken.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use verbatim_model::{FocusReaderKind, OutpostId};

/// A live re-read's answer of gone, or the listener's replacement: the
/// pending live re-reads they name end, and nothing is said. Any other
/// input does nothing here.
pub(crate) fn ended(
    state: &mut crate::SrState,
    input: &verbatim_model::Input,
) -> Vec<verbatim_model::Effect> {
    match input {
        verbatim_model::Input::LiveRereadGone { outpost, request } => {
            state.live_rereads.answered(*outpost, *request);
        }
        verbatim_model::Input::ListenerReplaced => state.live_rereads.listener_replaced(),
        _ => {}
    }
    Vec::new()
}

/// A live re-read Core waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Pending {
    /// Its request number.
    pub(crate) request: u64,
    /// When that report was observed, on the run's shared clock, 0 when
    /// not known.
    pub(crate) observed_at_us: u64,
}

/// The pending live re-reads, one per focus reader, and how many Core has
/// asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LiveRereads {
    /// The next request number.
    next: u64,
    /// The pending live re-read of each focus reader.
    pending: BTreeMap<(OutpostId, FocusReaderKind), Pending>,
    /// How many live re-reads Core has asked for.
    count: u64,
}

impl LiveRereads {
    /// Records a live re-read of a report observed at `observed_at_us`,
    /// asked of the focus reader `reader` of outpost
    /// generation `outpost`, replacing the one pending there, and counts
    /// it; returns its request number.
    pub(crate) fn ask(
        &mut self,
        (outpost, reader): (OutpostId, FocusReaderKind),
        observed_at_us: u64,
    ) -> u64 {
        let request = self.next;
        self.next = self.next.saturating_add(1);
        self.count = self.count.saturating_add(1);
        self.pending.insert(
            (outpost, reader),
            Pending {
                request,
                observed_at_us,
            },
        );
        request
    }

    /// An answer to `request` came through outpost generation `outpost`:
    /// whether it is a pending live re-read's, which it ends.
    pub(crate) fn answered(&mut self, outpost: OutpostId, request: u64) -> bool {
        let key = self
            .pending
            .iter()
            .find(|((generation, _), pending)| *generation == outpost && pending.request == request)
            .map(|(key, _)| *key);
        key.is_some_and(|key| self.pending.remove(&key).is_some())
    }

    /// A newer focus was applied: every pending live re-read ends.
    pub(crate) fn end_all(&mut self) {
        self.pending.clear();
    }

    /// Outpost generation `outpost` ended: its focus readers' pending live
    /// re-reads end.
    pub(crate) fn outpost_ended(&mut self, outpost: OutpostId) {
        self.pending
            .retain(|(generation, _), _| *generation != outpost);
    }

    /// The listener was replaced: the pending live re-reads asked of it
    /// end.
    pub(crate) fn listener_replaced(&mut self) {
        self.pending
            .retain(|(_, reader), _| *reader != FocusReaderKind::Listener);
    }

    /// How many live re-reads are pending: work in flight for the idle
    /// barrier.
    pub(crate) fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// When the newest report set aside for a pending live re-read was
    /// observed, for the fake focus's skip rule, which counts it as
    /// applied.
    pub(crate) fn latest_observed_at(&self) -> Option<u64> {
        self.pending
            .values()
            .map(|pending| pending.observed_at_us)
            .max()
    }

    /// How many live re-reads Core has asked for.
    pub(crate) fn count(&self) -> u64 {
        self.count
    }

    /// The pending live re-read of each focus reader, for the generated
    /// test's invariants.
    #[cfg(test)]
    pub(crate) fn pending(&self) -> &BTreeMap<(OutpostId, FocusReaderKind), Pending> {
        &self.pending
    }
}

#[cfg(test)]
mod generated;
#[cfg(test)]
mod tests;
