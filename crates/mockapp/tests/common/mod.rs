//! Shared cross-process test harness: spawning `mockapp`, waiting for it to
//! announce readiness, finding its window, and sending stdin commands.
//! Every test gives its window a unique title so parallel test binaries
//! never collide when searching by title.
//!
//! `tests/common/mod.rs` is compiled fresh into every `tests/*.rs` binary
//! (that is how Cargo shares test helpers without turning this into its own
//! test target), so any one binary legitimately uses only part of this
//! surface; the module-wide allow below reflects that rather than papering
//! over genuinely dead code within a single binary.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use std::os::windows::io::AsRawHandle;

use windows::Win32::Foundation::{HWND, WAIT_OBJECT_0};
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::core::PCWSTR;

/// Joins this thread to a COM apartment, required before any MSAA call that
/// retrieves a cross-process `IAccessible` (`AccessibleObjectFromWindow`,
/// `AccessibleObjectFromEvent`): the marshaling those calls do to unpack the
/// provider's object needs the calling thread to already be in an
/// apartment, and a bare `cargo test` thread starts in none. Tolerates a
/// thread that already joined some apartment (e.g. a prior call on the same
/// thread), as a repeated `verbatim_uia::init_mta` is.
pub fn init_com() {
    // SAFETY: `CoInitializeEx` with no reserved pointer is always sound; a
    // failure other than "already initialized" would fail the caller's next
    // COM operation anyway, so it is safe to ignore here.
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_MULTITHREADED,
        );
    }
}

/// Generous per-wait ceiling so CI runners under load do not flake, while
/// still failing fast (rather than hanging) when something is genuinely
/// broken.
pub const WAIT_TIMEOUT: Duration = Duration::from_secs(15);

static TITLE_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Builds a per-test-unique window title, so concurrently running tests
/// never find each other's windows.
#[must_use]
pub fn unique_title(prefix: &str) -> String {
    let n = TITLE_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{n}", std::process::id())
}

/// The path to a fixture file under `tests/fixtures`.
#[must_use]
pub fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// A running `mockapp` child process: killed on drop so a failing assertion
/// (which unwinds past the rest of the test) never leaves a window behind
/// to confuse the next test or a developer's desktop.
pub struct MockApp {
    child: Child,
    stdin: ChildStdin,
    /// mockapp's stdout, line by line: `ready`, then the acknowledgements
    /// of commands that print one.
    lines: mpsc::Receiver<String>,
    /// The window title this instance was started with, for `find_window`.
    pub title: String,
}

impl Drop for MockApp {
    fn drop(&mut self) {
        // Already ended by `quit` or by the test, the kill has nothing to
        // do; otherwise it must work, or the window is left behind.
        let killed = self.child.kill();
        let ended = self.child.wait();
        if !std::thread::panicking() {
            if let Err(error) = &killed {
                assert!(
                    matches!(self.child.try_wait(), Ok(Some(_))),
                    "mockapp {} could not be ended: {error}",
                    self.child.id()
                );
            }
            ended.unwrap_or_else(|error| panic!("mockapp could not be reaped: {error}"));
        }
    }
}

impl MockApp {
    /// Sends one stdin command line (without the trailing newline) and
    /// waits for mockapp's acknowledgement that it has taken effect, events
    /// included, failing the test if mockapp rejects it (a node id the
    /// fixture lacks, say). Not for `stall` or `quit`, which have
    /// [`stall`](Self::stall) and [`quit`](Self::quit).
    pub fn send(&mut self, line: &str) {
        self.write_line(line);
        let acknowledged = self.next_line(&format!("{line:?}"), WAIT_TIMEOUT);
        assert_eq!(acknowledged, "applied", "mockapp's answer to {line:?}");
    }

    /// Sends `line` and returns mockapp's acknowledgement, whatever it is:
    /// for a test of the acknowledgement itself.
    pub fn send_answered(&mut self, line: &str) -> String {
        self.write_line(line);
        self.next_line(&format!("{line:?}"), WAIT_TIMEOUT)
    }

    /// Asks mockapp to quit and waits for it to exit, which it must do
    /// cleanly.
    pub fn quit(mut self) {
        self.write_line("quit");
        let handle = windows::Win32::Foundation::HANDLE(self.child.as_raw_handle());
        let timeout = u32::try_from(WAIT_TIMEOUT.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: the child's process handle, live while `self` owns it.
        let waited = unsafe { WaitForSingleObject(handle, timeout) };
        assert_eq!(
            waited, WAIT_OBJECT_0,
            "mockapp did not quit within {WAIT_TIMEOUT:?}"
        );
        let status = self
            .child
            .wait()
            .unwrap_or_else(|error| panic!("mockapp's exit could not be read: {error}"));
        assert!(status.success(), "mockapp quit with {status}");
    }

    /// Writes one line to mockapp's stdin, failing the test if mockapp is
    /// no longer reading it.
    fn write_line(&mut self, line: &str) {
        writeln!(self.stdin, "{line}")
            .and_then(|()| self.stdin.flush())
            .unwrap_or_else(|error| panic!("mockapp did not take {line:?}: {error}"));
    }

    /// The child process id, for scoping a `WinEventHook` to this instance.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Stalls mockapp's window thread for `duration` and returns once the
    /// thread has acknowledged that the stall began, so every cross-process
    /// call made from here on waits on it.
    pub fn stall(&mut self, duration: Duration) {
        self.write_line(&format!("stall {}", duration.as_millis()));
        let line = self.next_line("the stall to begin", WAIT_TIMEOUT);
        assert_eq!(line, "stall started", "mockapp's acknowledgement");
    }

    /// Waits for the stall begun by [`stall`](Self::stall) to end, and
    /// returns when it ended, in microseconds since the Unix epoch (the
    /// clock of [`now_us`]). `stall` is the stall's length, which the wait
    /// allows on top of [`WAIT_TIMEOUT`].
    pub fn stall_ended(&mut self, stall: Duration) -> u64 {
        let line = self.next_line("the stall to end", stall + WAIT_TIMEOUT);
        line.strip_prefix("stall ended ")
            .and_then(|micros| micros.parse().ok())
            .unwrap_or_else(|| panic!("mockapp acknowledged {line:?}, not the stall's end"))
    }

    /// The next line mockapp prints, waiting at most `timeout`.
    fn next_line(&self, what: &str, timeout: Duration) -> String {
        self.lines
            .recv_timeout(timeout)
            .unwrap_or_else(|_| panic!("mockapp did not acknowledge {what} within {timeout:?}"))
    }
}

/// Microseconds since the Unix epoch, the clock mockapp's `stall ended`
/// acknowledgement and the outpost's event timings use, so a time read here
/// orders against theirs.
#[must_use]
pub fn now_us() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros(),
    )
    .unwrap_or(u64::MAX)
}

/// Spawns `mockapp --fixture <fixture> --backend <backend> --title <title>`,
/// waits (with [`WAIT_TIMEOUT`]) for it to print `ready`, and returns the
/// guard. Panics with a clear message if the process never becomes ready.
#[must_use]
pub fn spawn(fixture: &str, backend: &str, title: &str) -> MockApp {
    let exe = env!("CARGO_BIN_EXE_mockapp");
    let mut child = Command::new(exe)
        .arg("--fixture")
        .arg(fixture_path(fixture))
        .arg("--backend")
        .arg(backend)
        .arg("--title")
        .arg(title)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap_or_else(|error| panic!("failed to spawn mockapp ({exe}): {error}"));

    let stdin = child.stdin.take().expect("mockapp stdin was piped");
    let stdout = child.stdout.take().expect("mockapp stdout was piped");

    // A background thread reads stdout lines, for as long as mockapp runs,
    // so every wait for one can honor a timeout instead of blocking forever
    // on a hung child.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { return };
            if tx.send(line.trim().to_owned()).is_err() {
                return;
            }
        }
    });

    match rx.recv_timeout(WAIT_TIMEOUT) {
        Ok(line) if line == "ready" => {}
        Ok(other) => panic!("mockapp's first stdout line was {other:?}, not \"ready\""),
        Err(_) => {
            let _ = child.kill();
            panic!(
                "mockapp did not print \"ready\" within {WAIT_TIMEOUT:?} (title {title:?}, fixture {fixture:?})"
            );
        }
    }

    MockApp {
        child,
        stdin,
        lines: rx,
        title: title.to_owned(),
    }
}

/// The top-level window titled exactly `title`. mockapp creates its window
/// before it prints `ready`, so a window [`spawn`] returned is there at
/// once; the test fails if it is not.
#[must_use]
pub fn find_window(title: &str) -> HWND {
    let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `wide` is a live, NUL-terminated buffer for the call.
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::FindWindowW(PCWSTR::null(), PCWSTR(wide.as_ptr()))
    }
    .unwrap_or_else(|error| panic!("no window is titled {title:?} once mockapp is ready: {error}"))
}

/// A real outpost in the test process, watching mockapp.
pub mod outpost;

/// The real tree view of `tests/fixtures/tree_view.json`.
pub mod tree_view;

/// mockapp's provider-side hit counters (`src/hits.rs`), compiled into the
/// tests too, so both sides share one method list and one pair of message
/// numbers.
#[path = "../../src/hits.rs"]
pub mod hits;

/// Zeroes mockapp's hit counters.
pub fn reset_hits(hwnd: HWND) {
    // SAFETY: a message with no pointers, answered by mockapp's window
    // procedure.
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::SendMessageW(
            hwnd,
            hits::WM_HITS_RESET,
            None,
            None,
        );
    }
}

/// mockapp's hit counters that are not zero, by method name, in counter
/// order.
#[must_use]
pub fn read_hits(hwnd: HWND) -> Vec<(&'static str, u32)> {
    hits::Method::ALL
        .iter()
        .enumerate()
        .filter_map(|(index, method)| {
            // SAFETY: as in `reset_hits`; the index travels as a plain
            // integer.
            let count = unsafe {
                windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                    hwnd,
                    hits::WM_HITS_READ,
                    Some(windows::Win32::Foundation::WPARAM(index)),
                    None,
                )
            }
            .0;
            let count = u32::try_from(count).unwrap_or(u32::MAX);
            (count != 0).then(|| (method.name(), count))
        })
        .collect()
}

/// Sends `line` to mockapp and waits until it has taken effect, then zeroes
/// the hit counters, so what is measured next starts from nothing.
pub fn apply(app: &mut MockApp, hwnd: HWND, line: &str) {
    app.send(line);
    reset_hits(hwnd);
}
