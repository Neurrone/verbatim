//! What both of Verbatim's readers, an outpost and the focus listener,
//! apply before reading: NVDA's class verdict and the class name
//! normalization it is judged on ([`class`]), NVDA's acceptance test
//! ([`acceptance`]), a window's handle that is never 0 ([`Window`]), and
//! the constants both readers wait and judge by. A
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

use std::time::Duration;

pub mod acceptance;
pub mod class;
pub mod in_flight;
mod window;

pub use window::Window;

/// The console host's window class. A console window's provider is the
/// console host, the process of the window's thread, while
/// `GetWindowThreadProcessId` names the console's client as its owner.
pub const CONSOLE_WINDOW_CLASS: &str = "ConsoleWindowClass";

/// How long a slow call into an application is waited for before it is
/// abandoned because the user has moved on to a window of the same
/// application on another UI thread: NVDA's `MIN_CORE_ALIVE_TIMEOUT`,
/// after which NVDA cancels a slow call once the foreground has changed.
/// One application can own windows on several threads (File Explorer's
/// folder windows, the taskbar, and the Alt+Tab switcher), and those must
/// not wait behind a slow window the user has left. An outpost's reducer
/// judges its programs by it (`books/dev/src/explanation/design/focus-pipeline.md`, section
/// 18.3), and the focus listener's reading threads their reads, each
/// judged by `verbatim_win32::window::front_is_another_thread_of_its_application`.
pub const MOVED_ON_GRACE: Duration = Duration::from_millis(500);
