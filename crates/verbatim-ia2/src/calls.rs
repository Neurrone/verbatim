//! The count of cross-process calls this crate makes, per thread
//! (`docs/performance.md`, "What counts as a call").
//!
//! A call counts when it reaches the application's process: an
//! `IAccessible` method (`accName`, `accValue`, `accRole`, `accState`,
//! `accDescription`, `accKeyboardShortcut`, `accLocation`, `accParent`,
//! `accChildCount`, `accNavigate`, `accFocus`, `accSelection`,
//! `accDefaultAction`, `accDoDefaultAction`), `IAccIdentity`'s identity
//! string, the acquisitions `AccessibleObjectFromEvent`,
//! `AccessibleObjectFromWindow`, `AccessibleChildren`, and
//! `WindowFromAccessibleObject`, a `QueryInterface` for any interface but
//! `IUnknown` on an object from another process, and a window message sent
//! to one of the application's windows (a list view's or tree view's `LVM_`
//! and `TVM_` messages). One API call counts once, however many round trips
//! it makes inside (`AccessibleChildren` and `WindowFromAccessibleObject`
//! each make several); the provider-side counters in `mockapp` show those.
//!
//! `QueryInterface` is counted every time because COM answers it from its
//! proxy only when the proxy already holds that interface, which Verbatim
//! cannot see; asking for `IUnknown`, the object's identity, is always
//! answered locally and is not counted. Nor are local window functions on
//! the application's window handles (`IsWindow`, `GetClassName`,
//! `GetAncestor`, `GetWindow`, `IsWindowVisible`, `IsChild`,
//! `GetGUIThreadInfo`), which never reach its message loop, nor the
//! reference counting COM does when an object is kept or released.
//!
//! The count is per thread, since each outpost has one worker thread making
//! all of its calls: the worker takes the count around each entry it handles
//! ([`take`]) and sends it with the entry's event or reply.

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
        count(CallKind::Msaa);
        count(CallKind::Msaa);
        count(CallKind::WindowMessage);
        assert_eq!(
            take(),
            CallCounts {
                uia: 0,
                msaa: 2,
                window_messages: 1
            }
        );
        assert!(take().is_empty());
    }
}
