//! The count of cross-process calls this crate makes, per thread
//! (`books/dev/src/reference/performance.md`, "What counts as a call").
//!
//! A call counts when it reaches the application's process: a UIA client
//! method that the application's provider answers, such as
//! `GetFocusedElementBuildCache`, `ElementFromHandleBuildCache`, a tree
//! walker's `*BuildCache` step, `BuildUpdatedCache`, `FindFirstBuildCache`,
//! `GetCurrentPattern`, a pattern's method, or `NormalizeElementBuildCache`;
//! and a window message sent to one of its windows, which is what the raw
//! probe is (one `WM_GETOBJECT`). One API call counts
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
//!
//! Beside the calls, each thread counts its failed cached reads: a
//! `Cached*` read that UIA answers with a failure, which it does for a
//! property or pattern the element's cache request did not hold, raising
//! and reporting an error inside itself (`books/dev/src/explanation/design/uia-shapes.md`,
//! section 3.6). A failed read is not a call, so it is counted apart from
//! [`CallCounts`], and kept in every build profile. Its readers gather it
//! per thread into counters they own: a [`Registration`] for UIA's
//! callback threads, the listener's reader, and the outpost's worker.
//!
//! Of those calls, each thread also counts the ones that connect to a
//! window by its handle (`ElementFromHandle`, `ElementFromHandleBuildCache`,
//! `GetFocusedElementBuildCache`), in
//! [`CallCounts::connections`], which the focus listener must keep at zero
//! (`books/dev/src/explanation/design/listener-without-connections.md`, section 2).
//!
//! [`Registration`]: crate::Registration

use std::cell::Cell;

use verbatim_model::{CallCounts, CallKind};

thread_local! {
    /// This thread's calls since the last [`take`].
    static COUNTS: Cell<CallCounts> = const {
        Cell::new(CallCounts {
            uia: 0,
            msaa: 0,
            window_messages: 0,
            connections: 0,
        })
    };
}

thread_local! {
    /// This thread's failed cached reads since the last
    /// [`take_failed_reads`].
    static FAILED_READS: Cell<u32> = const { Cell::new(0) };
}

/// Counts one failed cached read on this thread.
pub(crate) fn count_failed_read() {
    FAILED_READS.with(|failed| failed.set(failed.get().saturating_add(1)));
}

/// This thread's failed cached reads since the last take, resetting them
/// to zero.
#[must_use]
pub fn take_failed_reads() -> u32 {
    FAILED_READS.with(Cell::take)
}

/// This thread's failed cached reads since the last take, leaving them
/// counted: for the reads one step made, by the difference between two
/// looks.
#[must_use]
pub fn peek_failed_reads() -> u32 {
    FAILED_READS.with(Cell::get)
}

/// Counts one cross-process call of `kind` on this thread, once this
/// thread's call gate lets it be made (`verbatim_win32::calls`).
///
/// # Errors
///
/// [`CallError::Abandoned`](verbatim_win32::calls::CallError::Abandoned)
/// once the outpost worker making it was abandoned: the call is not made,
/// nor counted.
#[track_caller]
pub fn count(kind: CallKind) -> Result<(), verbatim_win32::calls::CallError> {
    // No counted call is made under one of the outpost's locks, checked in
    // debug builds.
    verbatim_win32::held_locks::check("a counted call into an application");
    verbatim_win32::calls::check()?;
    COUNTS.with(|counts| {
        let mut current = counts.get();
        current.record(kind);
        counts.set(current);
    });
    Ok(())
}

/// Counts, on this thread, that the call just counted under its kind
/// connected to a window by its handle ([`CallCounts::connections`]).
pub(crate) fn count_connection() {
    COUNTS.with(|counts| {
        let mut current = counts.get();
        current.record_connection();
        counts.set(current);
    });
}

/// This thread's calls since the last take, resetting them to zero.
#[must_use]
pub fn take() -> CallCounts {
    COUNTS.with(Cell::take)
}

/// This thread's calls since the last take, leaving them counted: for
/// logging the calls one step of an entry made, by the difference between
/// two looks.
#[must_use]
pub fn peek() -> CallCounts {
    COUNTS.with(Cell::get)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_returns_and_resets_this_threads_counts() {
        let _ = take();
        count(CallKind::Uia).expect("no gate on this thread");
        count(CallKind::WindowMessage).expect("no gate on this thread");
        count_connection();
        assert_eq!(
            take(),
            CallCounts {
                uia: 1,
                msaa: 0,
                window_messages: 1,
                connections: 1
            }
        );
        assert!(take().is_empty());
        count(CallKind::Msaa).expect("no gate on this thread");
        assert_eq!(peek().msaa, 1);
        assert_eq!(take().msaa, 1);
        // Another thread's calls are its own.
        std::thread::spawn(|| count(CallKind::Uia).expect("no gate on this thread"))
            .join()
            .expect("the thread ran");
        assert!(take().is_empty());
    }

    #[test]
    fn take_failed_reads_returns_and_resets_this_threads_count() {
        let _ = take_failed_reads();
        count_failed_read();
        count_failed_read();
        assert_eq!(peek_failed_reads(), 2);
        assert_eq!(take_failed_reads(), 2);
        assert_eq!(take_failed_reads(), 0);
        // Another thread's failed reads are its own, and are not calls.
        std::thread::spawn(count_failed_read)
            .join()
            .expect("the thread ran");
        assert_eq!(take_failed_reads(), 0);
        assert!(take().is_empty());
    }
}
