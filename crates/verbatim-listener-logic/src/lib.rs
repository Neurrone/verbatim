//! The focus listener's pure logic (decision D13): what its reading
//! threads, its read scheduler and its foreground watch decide, with no
//! Windows type, no call and no clock, so every ordering of inputs is a
//! replay test. The listener's crate, `verbatim-listener`, runs it on its
//! threads and makes every call.
//!
//! - [`focus`]: the vocabulary a focus event is judged and scheduled in,
//!   and the reasons an event is dropped.
//! - [`schedule`]: the read scheduler, which reading thread reads which
//!   application's event and when a read is abandoned, and the menu end's
//!   grace.
//! - [`foreground`]: what the listener learns of the window in the
//!   foreground for the keyboard hook.

pub mod focus;
pub mod foreground;
pub mod schedule;
