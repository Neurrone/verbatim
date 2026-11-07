//! The focus listener's pure logic (decision D13): what its reading
//! threads, its read scheduler and its foreground watch decide, with no
//! Windows type, no call and no clock, so every ordering of inputs is a
//! replay test. The listener's crate, `verbatim-listener`, runs it on its
//! threads and makes every call.
//!
//! - [`checks`]: what a focus event goes through before its read, NVDA's
//!   checks in their order, as a pure step function.
//! - [`focus`]: the vocabulary a focus event is judged and scheduled in,
//!   and the reasons an event is dropped.
//! - [`schedule`]: the read scheduler, which reading thread reads which
//!   application's event and when a read is abandoned, and the menu end's
//!   grace.
//! - [`foreground`]: what the listener learns of the window in the
//!   foreground for the keyboard hook.

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

pub mod checks;
pub mod focus;
pub mod foreground;
pub mod schedule;
