//! The time limits a slow application is waited for by, shared by the
//! outposts, their logic crate, the focus listener and Core: each limit is
//! longer than everything inside it, plus a margin, and each nesting is
//! stated by a compile-time assertion, so a later change cannot break one
//! silently. A slow application is waited for, as NVDA's watchdog waits
//! for its core (`docs/nvda/main-loop-and-watchdog.md`), rather than cut
//! off.

use std::time::Duration;

/// How long one UI Automation request may take: the transaction timeout
/// UIA's first client in a process sets, a setting of the whole process,
/// so it bounds the listener's reads and every outpost call alike. One
/// UIA method can make several requests, each bounded by it.
pub const UIA_TRANSACTION_TIMEOUT: Duration = Duration::from_secs(10);

/// The margin above the slowest call a limit holds, for the work around
/// it and the answer's way back.
const MARGIN: Duration = Duration::from_secs(1);

/// The deadline of one task of an outpost's, a program, and of Core's wait
/// for most of their answers: one UIA request, [`UIA_TRANSACTION_TIMEOUT`],
/// and a margin of a second. It bounds MSAA calls and window messages too,
/// which have no time limit of their own: the deadline and the judgement
/// that a window is hung are their only bounds. A task of several slow
/// calls can still pass it, as a core cycle of several slow calls passes
/// NVDA's watchdog (`docs/nvda/main-loop-and-watchdog.md`); only one
/// request is promised to end by its own timeout.
pub const HANDLING_DEADLINE: Duration = UIA_TRANSACTION_TIMEOUT.saturating_add(MARGIN);

/// How long a walk in the window of the focus before it is waited for
/// before it is published from its progress, and how long the focus
/// listener's NVDA checks of a read may take: the bound both readers' walks
/// share in the same top-level window, the outpost's and the listener's.
pub const SAME_WINDOW_BOUND: Duration = Duration::from_secs(2);

/// The deadline of work whose walk or checks are a [`SAME_WINDOW_BOUND`]
/// budget checked between calls, with no publication of their own beyond
/// it: an outpost's focus-now query, its report of a listener read, and
/// the listener's read from its start. The budget, one request and the
/// margin, so the last call begun under the budget ends by its own timeout
/// before the deadline.
pub const BOUNDED_WALK_DEADLINE: Duration = HANDLING_DEADLINE.saturating_add(SAME_WINDOW_BOUND);

// A task holds one request and the margin.
const _: () = assert!(HANDLING_DEADLINE.as_micros() > UIA_TRANSACTION_TIMEOUT.as_micros());
// A bounded walk's deadline holds its budget, one request and the margin.
const _: () = assert!(
    BOUNDED_WALK_DEADLINE.as_micros()
        >= SAME_WINDOW_BOUND.as_micros() + UIA_TRANSACTION_TIMEOUT.as_micros() + MARGIN.as_micros()
);
