//! What both of Verbatim's readers, an outpost and the focus listener,
//! apply before reading: NVDA's class verdict and the class name
//! normalization it is judged on ([`class`]), NVDA's acceptance test
//! ([`acceptance`]), and the constants both readers wait and judge by. A
//! rule both apply is defined here once, since the two readers' crates may
//! not depend on each other and no test could tie two copies (decision
//! D14, "Where the rules run"). Pure: no Windows dependency, no call and
//! no clock.

#![forbid(unsafe_code)]

use std::time::Duration;

pub mod acceptance;
pub mod class;

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
/// judges its programs by it (`docs/design/focus-pipeline.md`, section
/// 18.3), and the focus listener's reading threads their reads, each
/// judged by `verbatim_win32::window::front_is_another_thread_of_its_application`.
pub const MOVED_ON_GRACE: Duration = Duration::from_millis(500);

/// How long an outpost waits for a UIA focus after a focus `WinEvent`
/// before it reads the focus itself: the bound of its reducer's gap record.
/// The focus listener counts a nearest window's lookup that timed out in
/// less than this as failing fast, by definition: a lookup that took this
/// long has held its application's later events as long as the record
/// waits for a UIA focus, and is taken as the application not answering
/// in time. The reasons for a second are given with the gap record's bound
/// (the outpost reducer's `gap_record::BOUND_US`).
pub const UIA_FOCUS_WAIT: Duration = Duration::from_secs(1);
