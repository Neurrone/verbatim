//! The focus listener's pure logic (decision D13): what its check
//! threads, its reading threads and its foreground watch decide, with no
//! Windows type, no call and no clock, so every ordering of inputs is a
//! replay test. The listener's crate, `verbatim-listener`, runs it on its
//! threads and makes every call.
//!
//! - [`check`]: the calls a UI Automation event's check makes at delivery
//!   on its application's check thread, as a pure step function.
//! - [`check_threads`]: the check threads' records, one per application,
//!   and the idle barrier's wait for them.
//! - [`kept`]: the UI Automation elements the listener keeps for its tells.
//! - [`schedule`]: the reading threads' schedule, which thread runs which
//!   of Core's requests, and Cancel.
//! - [`foreground`]: what the listener learns of the window in the
//!   foreground for the keyboard hook, and the pending foreground that
//!   holds the MSAA tells behind it.
//! - [`tell`]: what the listener decides of a `WinEvent` before it tells
//!   Core of it: NVDA's local drops, the window a tell names and its
//!   facts.

// Non-test code never panics, overflows or truncates silently: arithmetic
// is checked or saturating, indexing goes through `get`, and casts through
// `From` or `TryFrom`. Tests are exempt, since a panic is how a test fails.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::exit,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::panic_in_result_fn,
    clippy::string_slice,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    clippy::unwrap_in_result,
    clippy::unwrap_used
)]
#![cfg_attr(
    test,
    allow(
        clippy::arithmetic_side_effects,
        clippy::as_conversions,
        clippy::exit,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::panic_in_result_fn,
        clippy::string_slice,
        clippy::todo,
        clippy::unimplemented,
        clippy::unreachable,
        clippy::unwrap_in_result,
        clippy::unwrap_used
    )
)]

pub mod check;
pub mod check_threads;
pub mod foreground;
pub mod kept;
pub mod schedule;
pub mod tell;
