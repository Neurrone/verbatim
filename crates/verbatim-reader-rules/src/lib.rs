//! What both of Verbatim's readers, an outpost and the focus listener,
//! apply before reading: NVDA's class verdict and the class name
//! normalization it is judged on ([`class`]), NVDA's acceptance test
//! ([`acceptance`]), the title check before a focus reader reuses its
//! ancestry cache ([`ancestry_cache`]), a window's handle that is never 0 ([`Window`]), the
//! kept verdicts on a window's provider ([`verdicts`]), NVDA's limit on a
//! thread's events ([`NewestPerThread`]), and
//! the console window's class. A
//! rule both apply is defined here once, since the two readers' crates may
//! not depend on each other and no test could tie two copies (decision
//! D14, "Where the rules run"). Pure: no Windows dependency, no call and
//! no clock.

#![forbid(unsafe_code)]
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

pub mod acceptance;
pub mod ancestry_cache;
pub mod class;
mod newest_per_thread;
pub mod verdicts;
mod window;

pub use newest_per_thread::{EVENTS_PER_THREAD, NewestPerThread};
pub use window::Window;

/// The console host's window class. A console window's provider is the
/// console host, the process of the window's thread, while
/// `GetWindowThreadProcessId` names the console's client as its owner.
pub const CONSOLE_WINDOW_CLASS: &str = "ConsoleWindowClass";
