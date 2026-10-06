//! The count of cross-process calls this crate makes, per thread
//! (`docs/performance.md`, "What counts as a call").
//!
//! A call counts when it reaches the application's process: a UIA client
//! method that the application's provider answers, such as
//! `GetFocusedElementBuildCache`, `ElementFromHandleBuildCache`, a tree
//! walker's `*BuildCache` step, `BuildUpdatedCache`, `FindFirstBuildCache`,
//! `GetCurrentPattern`, a pattern's method, or `NormalizeElementBuildCache`;
//! and a window message sent to one of its windows, which is what
//! `UiaHasServerSideProvider` is (one `WM_GETOBJECT`). One API call counts
//! once, however many round trips UIA makes inside it; the provider-side
//! counters in `mockapp` show those.
//!
//! Local work does not count: the `Cached*` getters and the cached snapshot
//! reads, `GetRuntimeId` (UIA keeps an element's runtime id with it),
//! creating a client, a cache request, a condition, or a tree walker,
//! setting a timeout, reading an element array, and taking or resolving an
//! agile reference. Nor do event subscriptions, which run on their own
//! threads and are not part of handling any one event.
//!
//! The count is per thread, since each outpost has one worker thread making
//! all of its calls: the worker takes the count around each entry it handles
//! ([`take`]) and sends it with the entry's event or reply. Other threads
//! that use this crate, the subscriptions and the listener, count too and
//! never take, which costs nothing.

use std::cell::Cell;

use verbatim_model::{CallCounts, CallKind};

thread_local! {
    /// This thread's calls since the last [`take`].
    static COUNTS: Cell<CallCounts> = const {
        Cell::new(CallCounts {
            uia: 0,
            msaa: 0,
            window_messages: 0,
        })
    };
}

/// Counts one cross-process call of `kind` on this thread.
pub fn count(kind: CallKind) {
    COUNTS.with(|counts| {
        let mut current = counts.get();
        current.record(kind);
        counts.set(current);
    });
}

/// This thread's calls since the last take, resetting them to zero.
#[must_use]
pub fn take() -> CallCounts {
    COUNTS.with(Cell::take)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_returns_and_resets_this_threads_counts() {
        let _ = take();
        count(CallKind::Uia);
        count(CallKind::WindowMessage);
        assert_eq!(
            take(),
            CallCounts {
                uia: 1,
                msaa: 0,
                window_messages: 1
            }
        );
        assert!(take().is_empty());
        // Another thread's calls are its own.
        std::thread::spawn(|| count(CallKind::Uia))
            .join()
            .expect("the thread ran");
        assert!(take().is_empty());
    }
}
