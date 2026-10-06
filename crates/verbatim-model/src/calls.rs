//! Counts of cross-process calls, by kind (`docs/performance.md`).
//!
//! The backend crates count every call that reaches the application's
//! process as they make it; the outpost sends the counts for each event and
//! reply it publishes, and Core's latency ledger keeps them with the trace.
//! The types live here, with no counting of their own, so the outpost's
//! protocol, the control plane, and the ledger share one vocabulary.

use serde::{Deserialize, Serialize};

/// What kind of cross-process call was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CallKind {
    /// A UI Automation client call that the application's provider answers:
    /// a fetch, a tree-walker step, a pattern fetch or method.
    Uia,
    /// An MSAA call: an `IAccessible` method, an `IAccessible` acquisition
    /// such as `AccessibleObjectFromEvent`, or a `QueryInterface` on an
    /// object from another process.
    Msaa,
    /// A window message sent to one of the application's windows and
    /// answered by its window procedure, such as `WM_GETOBJECT` or a
    /// common control's `LVM_` and `TVM_` messages.
    WindowMessage,
}

/// How many cross-process calls of each kind one piece of work made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct CallCounts {
    /// UI Automation calls.
    pub uia: u32,
    /// MSAA calls.
    pub msaa: u32,
    /// Window messages.
    pub window_messages: u32,
}

impl CallCounts {
    /// Counts one call of `kind`. Saturates rather than overflowing, since a
    /// thread whose counts are never taken keeps adding.
    pub fn record(&mut self, kind: CallKind) {
        let count = match kind {
            CallKind::Uia => &mut self.uia,
            CallKind::Msaa => &mut self.msaa,
            CallKind::WindowMessage => &mut self.window_messages,
        };
        *count = count.saturating_add(1);
    }

    /// Every call, of whatever kind.
    #[must_use]
    pub fn total(&self) -> u32 {
        self.uia
            .saturating_add(self.msaa)
            .saturating_add(self.window_messages)
    }

    /// Whether no call was made.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }
}

impl std::ops::Add for CallCounts {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            uia: self.uia.saturating_add(other.uia),
            msaa: self.msaa.saturating_add(other.msaa),
            window_messages: self.window_messages.saturating_add(other.window_messages),
        }
    }
}

impl std::ops::AddAssign for CallCounts {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_add_by_kind_and_total() {
        let mut counts = CallCounts::default();
        assert!(counts.is_empty());
        counts.record(CallKind::Uia);
        counts.record(CallKind::Uia);
        counts.record(CallKind::WindowMessage);
        let other = CallCounts {
            msaa: 3,
            ..CallCounts::default()
        };
        let sum = counts + other;
        assert_eq!(
            sum,
            CallCounts {
                uia: 2,
                msaa: 3,
                window_messages: 1
            }
        );
        assert_eq!(sum.total(), 6);
    }

    #[test]
    fn a_count_saturates() {
        let mut counts = CallCounts {
            uia: u32::MAX,
            ..CallCounts::default()
        };
        counts.record(CallKind::Uia);
        assert_eq!(counts.uia, u32::MAX);
    }
}
