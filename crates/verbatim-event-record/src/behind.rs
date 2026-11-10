//! Pairs each process's MSAA focus events with its UI Automation focus
//! events, by window, to find a UI Automation focus that arrived late: a
//! reducer the writer thread runs, the one place that sees both streams in
//! arrival order. The rules are the table in the crate's guide.

use std::collections::{HashMap, VecDeque};

use verbatim_uia::UIA_TRANSACTION_TIMEOUT;

/// How far behind an arrival must be to write a `behind` line.
const LATE: u64 = 100_000;

/// A focus event as the reducer judges it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Focus {
    /// An MSAA focus, of a window whose provider is known.
    Msaa {
        /// `provider_process` of the event's window.
        process: u32,
        /// The event's window.
        window: isize,
        /// `idObject`.
        object: i32,
        /// `idChild`.
        child: i32,
    },
    /// A UI Automation focus on an element with its own window.
    Uia {
        /// The element's process, from its cache.
        process: u32,
        /// The element's native window.
        window: isize,
    },
}

/// One waiting MSAA focus.
#[derive(Clone, Copy, Debug)]
struct MsaaEntry {
    window: isize,
    object: i32,
    child: i32,
    at_us: u64,
}

/// One waiting UI Automation focus.
#[derive(Clone, Copy, Debug)]
struct UiaEntry {
    window: isize,
    at_us: u64,
}

/// Which of a process's two streams has focus events waiting for the
/// other's: a queue of one side only, so both sides waiting cannot be
/// written. Neither queue is ever empty.
#[derive(Debug)]
enum Waiting {
    Msaa(VecDeque<MsaaEntry>),
    Uia(VecDeque<UiaEntry>),
}

impl Waiting {
    /// This value with every entry older than the transaction timeout
    /// dropped, `None` when none remains.
    fn without_expired(self, at_us: u64) -> Option<Self> {
        let expired = |entry_at: u64| at_us.saturating_sub(entry_at) > timeout_us();
        match self {
            Self::Msaa(mut queue) => {
                while queue.front().is_some_and(|e| expired(e.at_us)) {
                    queue.pop_front();
                }
                (!queue.is_empty()).then_some(Self::Msaa(queue))
            }
            Self::Uia(mut queue) => {
                while queue.front().is_some_and(|e| expired(e.at_us)) {
                    queue.pop_front();
                }
                (!queue.is_empty()).then_some(Self::Uia(queue))
            }
        }
    }
}

/// `UIA_TRANSACTION_TIMEOUT` in microseconds.
fn timeout_us() -> u64 {
    u64::try_from(UIA_TRANSACTION_TIMEOUT.as_micros()).unwrap_or(u64::MAX)
}

/// One `Waiting` per process with focus events waiting.
#[derive(Debug, Default)]
pub(crate) struct Pairs {
    waiting: HashMap<u32, Waiting>,
}

impl Pairs {
    /// Takes the focus event `focus`, which arrived at `at_us`, and
    /// returns how many milliseconds a UI Automation focus it paired with
    /// arrived behind its MSAA focus, when that is more than the limit.
    pub(crate) fn arrive(&mut self, focus: Focus, at_us: u64) -> Option<u64> {
        let process = match focus {
            Focus::Msaa { process, .. } | Focus::Uia { process, .. } => process,
        };
        let current = self
            .waiting
            .remove(&process)
            .and_then(|waiting| waiting.without_expired(at_us));
        let (next, late) = step(current, focus, at_us);
        if let Some(next) = next {
            self.waiting.insert(process, next);
        }
        late
    }
}

/// The table's rule for `focus` arriving at `at_us` in state `current`,
/// the entries already judged for expiry: the state after it, and the
/// milliseconds behind when a `behind` line is written.
fn step(current: Option<Waiting>, focus: Focus, at_us: u64) -> (Option<Waiting>, Option<u64>) {
    match (current, focus) {
        (
            None,
            Focus::Msaa {
                window,
                object,
                child,
                ..
            },
        ) => (
            Some(Waiting::Msaa(VecDeque::from([MsaaEntry {
                window,
                object,
                child,
                at_us,
            }]))),
            None,
        ),
        (None, Focus::Uia { window, .. }) => (
            Some(Waiting::Uia(VecDeque::from([UiaEntry { window, at_us }]))),
            None,
        ),
        (
            Some(Waiting::Msaa(mut queue)),
            Focus::Msaa {
                window,
                object,
                child,
                ..
            },
        ) => {
            let repeats = queue.back().is_some_and(|newest| {
                newest.window == window && newest.object == object && newest.child == child
            });
            if !repeats {
                queue.push_back(MsaaEntry {
                    window,
                    object,
                    child,
                    at_us,
                });
            }
            (Some(Waiting::Msaa(queue)), None)
        }
        (Some(Waiting::Msaa(mut queue)), Focus::Uia { window, .. }) => {
            let Some(at) = queue.iter().position(|entry| entry.window == window) else {
                return (
                    Some(Waiting::Uia(VecDeque::from([UiaEntry { window, at_us }]))),
                    None,
                );
            };
            let paired = queue
                .drain(..=at)
                .next_back()
                .expect("the position is in the queue");
            let late_us = at_us.saturating_sub(paired.at_us);
            let rest = (!queue.is_empty()).then_some(Waiting::Msaa(queue));
            (rest, (late_us > LATE).then_some(late_us / 1000))
        }
        (Some(Waiting::Uia(mut queue)), Focus::Uia { window, .. }) => {
            queue.push_back(UiaEntry { window, at_us });
            (Some(Waiting::Uia(queue)), None)
        }
        (
            Some(Waiting::Uia(mut queue)),
            Focus::Msaa {
                window,
                object,
                child,
                ..
            },
        ) => {
            let Some(at) = queue.iter().position(|entry| entry.window == window) else {
                return (
                    Some(Waiting::Msaa(VecDeque::from([MsaaEntry {
                        window,
                        object,
                        child,
                        at_us,
                    }]))),
                    None,
                );
            };
            queue.drain(..=at);
            ((!queue.is_empty()).then_some(Waiting::Uia(queue)), None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROCESS: u32 = 7;

    fn msaa(window: isize) -> Focus {
        Focus::Msaa {
            process: PROCESS,
            window,
            object: -4,
            child: 0,
        }
    }

    fn uia(window: isize) -> Focus {
        Focus::Uia {
            process: PROCESS,
            window,
        }
    }

    /// What waits for the process: the side and how many entries.
    fn waiting(pairs: &Pairs) -> Option<(&'static str, usize)> {
        match pairs.waiting.get(&PROCESS)? {
            Waiting::Msaa(queue) => Some(("msaa", queue.len())),
            Waiting::Uia(queue) => Some(("uia", queue.len())),
        }
    }

    #[test]
    fn with_nothing_waiting_an_msaa_focus_waits() {
        let mut pairs = Pairs::default();
        assert_eq!(pairs.arrive(msaa(1), 0), None);
        assert_eq!(waiting(&pairs), Some(("msaa", 1)));
    }

    #[test]
    fn with_nothing_waiting_a_uia_focus_waits() {
        let mut pairs = Pairs::default();
        assert_eq!(pairs.arrive(uia(1), 0), None);
        assert_eq!(waiting(&pairs), Some(("uia", 1)));
    }

    #[test]
    fn an_msaa_focus_that_repeats_the_newest_is_dropped() {
        let mut pairs = Pairs::default();
        pairs.arrive(msaa(1), 0);
        assert_eq!(pairs.arrive(msaa(1), 10), None);
        assert_eq!(waiting(&pairs), Some(("msaa", 1)));
    }

    #[test]
    fn an_msaa_focus_of_another_window_is_queued() {
        let mut pairs = Pairs::default();
        pairs.arrive(msaa(1), 0);
        assert_eq!(pairs.arrive(msaa(2), 10), None);
        assert_eq!(waiting(&pairs), Some(("msaa", 2)));
    }

    #[test]
    fn a_uia_focus_pairs_by_window_and_reports_a_late_arrival() {
        let mut pairs = Pairs::default();
        pairs.arrive(msaa(1), 0);
        pairs.arrive(msaa(2), 50_000);
        assert_eq!(pairs.arrive(uia(2), 250_000), Some(200));
        // The entry for window 1 is older, so it went with the pair.
        assert_eq!(waiting(&pairs), None);
    }

    #[test]
    fn a_uia_focus_within_the_limit_writes_no_line() {
        let mut pairs = Pairs::default();
        pairs.arrive(msaa(1), 0);
        assert_eq!(pairs.arrive(uia(1), 100_000), None);
        assert_eq!(waiting(&pairs), None);
    }

    #[test]
    fn a_uia_focus_with_no_entry_for_its_window_replaces_the_msaa_queue() {
        let mut pairs = Pairs::default();
        pairs.arrive(msaa(1), 0);
        assert_eq!(pairs.arrive(uia(2), 200_000), None);
        assert_eq!(waiting(&pairs), Some(("uia", 1)));
    }

    #[test]
    fn an_msaa_focus_pairs_with_a_waiting_uia_focus_and_writes_no_line() {
        let mut pairs = Pairs::default();
        pairs.arrive(uia(1), 0);
        pairs.arrive(uia(2), 10);
        assert_eq!(pairs.arrive(msaa(2), 900_000), None);
        assert_eq!(waiting(&pairs), None);
    }

    #[test]
    fn an_msaa_focus_with_no_entry_for_its_window_replaces_the_uia_queue() {
        let mut pairs = Pairs::default();
        pairs.arrive(uia(1), 0);
        assert_eq!(pairs.arrive(msaa(2), 10), None);
        assert_eq!(waiting(&pairs), Some(("msaa", 1)));
    }

    #[test]
    fn a_uia_focus_queues_behind_a_uia_focus() {
        let mut pairs = Pairs::default();
        pairs.arrive(uia(1), 0);
        assert_eq!(pairs.arrive(uia(2), 10), None);
        assert_eq!(waiting(&pairs), Some(("uia", 2)));
    }

    #[test]
    fn entries_older_than_the_transaction_timeout_expire_at_the_next_arrival() {
        let mut pairs = Pairs::default();
        pairs.arrive(msaa(1), 0);
        // Window 1's entry has expired, so this waits on its own.
        assert_eq!(pairs.arrive(uia(1), timeout_us() + 1), None);
        assert_eq!(waiting(&pairs), Some(("uia", 1)));
    }

    #[test]
    fn a_process_of_its_own_pairs_alone() {
        let mut pairs = Pairs::default();
        pairs.arrive(msaa(1), 0);
        let other = Focus::Uia {
            process: PROCESS + 1,
            window: 1,
        };
        assert_eq!(pairs.arrive(other, 200_000), None);
        assert_eq!(waiting(&pairs), Some(("msaa", 1)));
    }
}
