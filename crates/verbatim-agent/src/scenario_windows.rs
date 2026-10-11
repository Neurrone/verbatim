//! Proving a scenario left no window open, whatever its title has become:
//! [`Request::RecordWindows`](crate::protocol::Request::RecordWindows),
//! [`Request::WaitForNewWindowsGone`](crate::protocol::Request::WaitForNewWindowsGone),
//! and
//! [`Request::CloseAbandonedWindows`](crate::protocol::Request::CloseAbandonedWindows).
//!
//! The harness closes the windows a scenario opened by their titles, but a
//! window can lose the title it was found by: a File Explorer window whose
//! folder is deleted is retitled with the parent folder. So when a scenario
//! starts, the agent records every top-level window there is, its
//! baseline, and at the scenario's cleanup every shown window not in the
//! baseline must be gone.
//!
//! The baseline holds every top-level window, shown, hidden, or cloaked,
//! titled or not: a window that existed before the scenario started is
//! never the scenario's, whatever state it was in, so a window of the
//! owner's that was hidden or cloaked then (an application kept in the
//! notification area, a window on another virtual desktop) and is shown
//! later is neither reported nor closed. The windows checked against it
//! are the shown ones: visible, titled, unowned, and not cloaked, the
//! windows every condition and close request of the agent judges by
//! (`desktop::shown_top_level_windows`), so a window that is cloaked when
//! it closes, as the Settings app keeps its closed window, has gone, as it
//! has for [`desktop::close_windows`]. A window is identified by its
//! handle with its process and class ([`WindowIdentity`]), never by its
//! title.
//!
//! The baseline lives in the agent, which outlives each test process, and
//! belongs to the connection that recorded it. A connection that ends
//! while its baseline is still held, because its test process was killed
//! before its cleanup or its cleanup found windows left open, has
//! abandoned its scenario: the agent notes, then, the shown windows that
//! are not in the baseline, and the next scenario's start closes those,
//! and only those, by handle.

use std::collections::HashSet;
use std::ffi::c_void;
use std::io;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, PostMessageW, WM_CLOSE};

use crate::desktop;
use crate::protocol::WindowInfo;
use crate::wait;

/// What identifies a top-level window for as long as it exists: its
/// handle, with the process that owns it and its class, neither of which
/// changes while it lives, so a handle Windows gives another window once
/// this one is destroyed is not taken for it. Its title is no part of it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct WindowIdentity {
    /// The window's handle.
    window: u64,
    /// The OS process id of the process that owns it.
    pid: u32,
    /// Its class name.
    class: String,
}

impl WindowIdentity {
    /// The identity `window` has now.
    fn of(window: HWND) -> Self {
        let mut pid = 0u32;
        // SAFETY: tolerates any handle, writing 0 for an invalid one.
        unsafe {
            GetWindowThreadProcessId(window, Some(&raw mut pid));
        }
        Self {
            window: window.0 as u64,
            pid,
            class: desktop::class_name(window),
        }
    }

    /// The window's handle.
    fn handle(&self) -> HWND {
        HWND(usize::try_from(self.window).unwrap_or(0) as *mut c_void)
    }
}

/// What the agent knows of the scenario being run.
#[derive(Debug, Default)]
enum State {
    /// No scenario is being watched.
    #[default]
    Idle,
    /// A scenario is running: its baseline, recorded on `connection`.
    Watching {
        /// The connection that recorded it.
        connection: u64,
        /// Every top-level window there was when the scenario started.
        baseline: HashSet<WindowIdentity>,
    },
    /// The shown windows not in its baseline that a scenario had open when
    /// its connection ended with the baseline still held.
    Abandoned(Vec<WindowIdentity>),
}

/// The watch's state and its transitions, apart from the desktop, so they
/// can be tested with windows of a test's own making.
#[derive(Debug, Default)]
struct Watch {
    state: State,
}

impl Watch {
    /// Records `baseline` as the baseline of the scenario `connection`
    /// starts.
    fn record(&mut self, connection: u64, baseline: HashSet<WindowIdentity>) -> io::Result<()> {
        match self.state {
            State::Idle => {
                self.state = State::Watching {
                    connection,
                    baseline,
                };
                Ok(())
            }
            State::Watching { .. } => Err(io::Error::other(
                "the windows of a scenario are already being watched, for a connection still open: \
                 another run is using this agent",
            )),
            State::Abandoned(_) => Err(io::Error::other(
                "the windows a scenario abandoned have not been closed yet (CloseAbandonedWindows)",
            )),
        }
    }

    /// The baseline of the scenario `connection` is running.
    fn baseline(&self, connection: u64) -> io::Result<&HashSet<WindowIdentity>> {
        match &self.state {
            State::Watching {
                connection: watched,
                baseline,
            } if *watched == connection => Ok(baseline),
            _ => Err(io::Error::other(
                "this connection has recorded no windows (RecordWindows) to check against",
            )),
        }
    }

    /// Ends the watch of the scenario `connection` is running, every window
    /// it opened having gone.
    fn finish(&mut self, connection: u64) {
        if self.watched_by(connection) {
            self.state = State::Idle;
        }
    }

    /// Whether `connection` is running the scenario being watched.
    fn watched_by(&self, connection: u64) -> bool {
        matches!(self.state, State::Watching { connection: watched, .. } if watched == connection)
    }

    /// `connection` has ended: when it still held its scenario's baseline,
    /// the scenario is abandoned with the windows of `shown` not in it.
    /// Returns whether it was.
    fn connection_ended(
        &mut self,
        connection: u64,
        shown: impl FnOnce() -> Vec<WindowIdentity>,
    ) -> bool {
        let State::Watching {
            connection: watched,
            baseline,
        } = &self.state
        else {
            return false;
        };
        if *watched != connection {
            return false;
        }
        let left = new_windows(baseline, &shown());
        self.state = if left.is_empty() {
            State::Idle
        } else {
            State::Abandoned(left)
        };
        true
    }

    /// The windows an abandoned scenario left, none when no scenario was
    /// abandoned, or `None` while a scenario is being watched for a
    /// connection still open.
    fn abandoned(&self) -> Option<&[WindowIdentity]> {
        match &self.state {
            State::Idle => Some(&[]),
            State::Watching { .. } => None,
            State::Abandoned(left) => Some(left),
        }
    }

    /// Forgets the abandoned scenario, its windows having gone.
    fn forget_abandoned(&mut self) {
        if matches!(self.state, State::Abandoned(_)) {
            self.state = State::Idle;
        }
    }
}

/// The windows of `shown` that are not in `baseline`.
fn new_windows(
    baseline: &HashSet<WindowIdentity>,
    shown: &[WindowIdentity],
) -> Vec<WindowIdentity> {
    shown
        .iter()
        .filter(|window| !baseline.contains(*window))
        .cloned()
        .collect()
}

/// The windows of `left` that are still in `shown`.
fn still_open(left: &[WindowIdentity], shown: &[WindowIdentity]) -> Vec<WindowIdentity> {
    left.iter()
        .filter(|window| shown.contains(*window))
        .cloned()
        .collect()
}

/// The agent's watch, shared by every connection.
static WATCH: Mutex<Watch> = Mutex::new(Watch { state: State::Idle });

/// Signalled whenever a watch ends, for [`close_abandoned`] waiting on a
/// connection that has not yet seen its test process go.
static WATCH_ENDED: Condvar = Condvar::new();

fn lock() -> MutexGuard<'static, Watch> {
    WATCH.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The shown top-level windows' identities.
fn shown() -> Vec<WindowIdentity> {
    desktop::shown_top_level_windows()
        .into_iter()
        .map(WindowIdentity::of)
        .collect()
}

/// What a test is told of each of `windows`.
fn report(windows: &[WindowIdentity]) -> Vec<WindowInfo> {
    windows
        .iter()
        .map(|window| desktop::window_info(window.handle()))
        .collect()
}

/// Records every top-level window there is as the baseline of the scenario
/// `connection` starts, and returns how many there are.
///
/// # Errors
///
/// Returns an error while a scenario is being watched, or one abandoned
/// has not had its windows closed ([`close_abandoned`]).
pub(crate) fn record(connection: u64) -> io::Result<u32> {
    let baseline: HashSet<WindowIdentity> = desktop::all_top_level_windows()
        .into_iter()
        .map(WindowIdentity::of)
        .collect();
    let count = u32::try_from(baseline.len()).unwrap_or(u32::MAX);
    lock().record(connection, baseline)?;
    Ok(count)
}

/// Waits up to `timeout`, on window events, for every shown window not in
/// the baseline of the scenario `connection` is running to be gone, and
/// returns those still shown, closing none. The watch ends when none is
/// left; when some are, it is kept, so the scenario is abandoned with them
/// once its connection ends, and the next scenario's start closes them.
///
/// # Errors
///
/// Returns an error when `connection` recorded no baseline.
pub(crate) fn wait_for_new_gone(connection: u64, timeout: Duration) -> io::Result<Vec<WindowInfo>> {
    let mut watch = lock();
    let baseline = watch.baseline(connection)?;
    wait::until(|| new_windows(baseline, &shown()).is_empty(), timeout);
    let remaining = new_windows(baseline, &shown());
    if remaining.is_empty() {
        watch.finish(connection);
        WATCH_ENDED.notify_all();
    }
    Ok(report(&remaining))
}

/// Called as `connection` ends: when it still holds its scenario's
/// baseline, the scenario is abandoned with the shown windows not in it.
pub(crate) fn connection_ended(connection: u64) {
    let mut watch = lock();
    if watch.connection_ended(connection, shown) {
        tracing::warn!(
            connection,
            "a connection ended with its scenario's windows still watched; the next scenario's start closes what it left"
        );
        WATCH_ENDED.notify_all();
    }
}

/// Closes, by handle, the windows an abandoned scenario left that are
/// still shown, and waits up to `timeout`, on window events, for them to
/// go. A scenario still being watched for another connection is first
/// waited for, up to `timeout`, to end: a test process killed while the
/// agent was answering one of its requests is seen to have gone only once
/// that answer is written. Returns the windows found and asked to close,
/// and those still shown when the wait ended, the abandoned scenario being
/// forgotten once none is.
///
/// # Errors
///
/// Returns an error when another connection's scenario is still being
/// watched after `timeout`: another run is using this agent.
pub(crate) fn close_abandoned(timeout: Duration) -> io::Result<(Vec<WindowInfo>, Vec<WindowInfo>)> {
    let (mut watch, _) = WATCH_ENDED
        .wait_timeout_while(lock(), timeout, |watch| watch.abandoned().is_none())
        .unwrap_or_else(PoisonError::into_inner);
    let left = watch
        .abandoned()
        .ok_or_else(|| {
            io::Error::other(format!(
                "a scenario on another connection was still being watched after {timeout:?}: \
                 another run is using this agent"
            ))
        })?
        .to_vec();
    if left.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    // Closing by handle is safe here, unlike closing by title or image:
    // every window closed is one that was absent from the abandoned
    // scenario's baseline, which held every top-level window there was
    // when it started, and was shown when its connection ended. The
    // owner's own windows, such as the Windows Terminal running the suite,
    // were in that baseline, and a window the owner opened after the
    // connection ended was not among those noted then, so neither is ever
    // touched. Each window's identity is checked again just before its
    // close, so a handle Windows has since given another window is left
    // alone.
    let open = still_open(&left, &shown());
    let found = report(&open);
    for window in &open {
        if WindowIdentity::of(window.handle()) != *window {
            continue;
        }
        // SAFETY: PostMessageW carries no pointer and tolerates a window
        // that has since gone; its identity was checked just above.
        unsafe {
            let _ = PostMessageW(Some(window.handle()), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
    wait::until(|| still_open(&left, &shown()).is_empty(), timeout);
    let remaining = still_open(&left, &shown());
    if remaining.is_empty() {
        watch.forget_abandoned();
    }
    Ok((found, report(&remaining)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A window of the test's own making, identified by `window` alone.
    fn window(window: u64) -> WindowIdentity {
        WindowIdentity {
            window,
            pid: 100,
            class: "Test".to_owned(),
        }
    }

    #[test]
    fn a_window_opened_since_the_baseline_is_new_until_it_goes() {
        let mut watch = Watch::default();
        watch
            .record(1, [window(1), window(2)].into_iter().collect())
            .expect("records");
        let baseline = watch.baseline(1).expect("connection 1's baseline");
        assert_eq!(
            new_windows(baseline, &[window(1), window(3), window(2)]),
            [window(3)],
        );
        // The same handle in another process is another window.
        let reused = WindowIdentity {
            pid: 200,
            ..window(2)
        };
        assert_eq!(
            new_windows(baseline, &[window(1), reused.clone()]),
            [reused]
        );
        assert!(watch.baseline(2).is_err(), "only connection 1's");
    }

    #[test]
    fn with_no_new_window_shown_the_watch_finishes() {
        let mut watch = Watch::default();
        watch
            .record(1, [window(1), window(2)].into_iter().collect())
            .expect("records");
        // A baseline window that has closed since is not asked about.
        let baseline = watch.baseline(1).expect("connection 1's baseline");
        assert_eq!(new_windows(baseline, &[window(2)]), Vec::new());
        watch.finish(1);
        assert!(!watch.connection_ended(1, Vec::new), "nothing to abandon");
        assert_eq!(watch.abandoned(), Some(&[][..]));
        watch
            .record(2, std::iter::once(window(1)).collect())
            .expect("the next scenario records");
    }

    #[test]
    fn an_abandoned_scenario_leaves_only_the_windows_it_opened() {
        let mut watch = Watch::default();
        watch
            .record(1, [window(1), window(2)].into_iter().collect())
            .expect("records");
        assert_eq!(watch.abandoned(), None, "connection 1 is still running");
        assert!(watch.connection_ended(1, || vec![window(1), window(3)]));
        assert!(
            watch.record(2, HashSet::new()).is_err(),
            "nothing is recorded before the abandoned windows are closed"
        );
        let left = watch.abandoned().expect("abandoned").to_vec();
        assert_eq!(left, [window(3)]);
        // Window 4 opened after connection 1 ended, as the owner's own
        // would: it is never among the windows closed.
        assert_eq!(
            still_open(&left, &[window(1), window(4), window(3)]),
            [window(3)]
        );
        watch.forget_abandoned();
        watch
            .record(2, std::iter::once(window(1)).collect())
            .expect("the next scenario records");
    }
}
