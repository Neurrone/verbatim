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
/// which have no time limit of their own: the deadline is their only
/// bound, since a window found hung while one waits on it is told of and
/// waited for, never abandoned for its hang. A task of several slow
/// calls can still pass it, as a core cycle of several slow calls passes
/// NVDA's watchdog (`docs/nvda/main-loop-and-watchdog.md`); only one
/// request is promised to end by its own timeout.
pub const HANDLING_DEADLINE: Duration = UIA_TRANSACTION_TIMEOUT.saturating_add(MARGIN);

/// The time a focus read's checks around its walk may add to its one
/// request: NVDA's checks before the walk and the read's local calls.
const CHECKS: Duration = Duration::from_secs(2);

/// The deadline of a focus read: the focus listener's read from its start,
/// an outpost's focus read (`Query::ReadFocus`) and its report of a
/// listener read. [`HANDLING_DEADLINE`] and two seconds for the checks
/// around the walk, 13 seconds. Past it the read's thread is abandoned and
/// the focus is lost; nothing else ends a read early but UIA's own
/// timeout and the moved-on rule.
pub const FOCUS_READ_DEADLINE: Duration = HANDLING_DEADLINE.saturating_add(CHECKS);

// A task holds one request and the margin.
const _: () = assert!(HANDLING_DEADLINE.as_micros() > UIA_TRANSACTION_TIMEOUT.as_micros());
// A focus read's deadline holds a task's and the checks around its walk.
const _: () = assert!(FOCUS_READ_DEADLINE.as_micros() > HANDLING_DEADLINE.as_micros());
