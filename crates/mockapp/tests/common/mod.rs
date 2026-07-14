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
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;
use windows::core::PCWSTR;

/// Joins this thread to a COM apartment, required before any MSAA call that
/// retrieves a cross-process `IAccessible` (`AccessibleObjectFromWindow`,
/// `AccessibleObjectFromEvent`): the marshaling those calls do to unpack the
/// provider's object needs the calling thread to already be in an
/// apartment, and a bare `cargo test` thread starts in none. Tolerates a
/// thread that already joined some apartment (e.g. a prior call on the same
/// thread), matching `verbatim_uia::init_mta`'s idempotence.
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
    /// The window title this instance was started with, for `find_window`.
    pub title: String,
}

impl Drop for MockApp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl MockApp {
    /// Sends one stdin command line (a `focus`, `set-name`, `set-value`, or
    /// `quit` command, without the trailing newline).
    pub fn send(&mut self, line: &str) {
        let _ = writeln!(self.stdin, "{line}");
        let _ = self.stdin.flush();
    }

    /// The child process id, for scoping a `WinEventHook` to this instance.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.child.id()
    }
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

    // A background thread reads stdout lines so the wait below can honor a
    // timeout instead of blocking forever on a hung child.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut lines = BufReader::new(stdout).lines();
        if let Some(Ok(line)) = lines.next() {
            let _ = tx.send(line);
        }
    });

    match rx.recv_timeout(WAIT_TIMEOUT) {
        Ok(line) if line.trim() == "ready" => {}
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
        title: title.to_owned(),
    }
}

/// Polls `FindWindowW` for a top-level window titled exactly `title`,
/// retrying until [`WAIT_TIMEOUT`] elapses. Panics with a clear message on
/// timeout, since every caller needs the handle to proceed.
#[must_use]
pub fn find_window(title: &str) -> HWND {
    let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let deadline = Instant::now() + WAIT_TIMEOUT;
    loop {
        // SAFETY: `wide` is a live, NUL-terminated buffer for the call.
        let found = unsafe {
            windows::Win32::UI::WindowsAndMessaging::FindWindowW(
                PCWSTR::null(),
                PCWSTR(wide.as_ptr()),
            )
        };
        if let Ok(hwnd) = found {
            return hwnd;
        }
        assert!(
            Instant::now() < deadline,
            "window titled {title:?} did not appear within {WAIT_TIMEOUT:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Polls `condition` until it returns `true` or [`WAIT_TIMEOUT`] elapses,
/// panicking with `message` on timeout. Used for event-delivery assertions,
/// where the client-side registration callback runs asynchronously.
pub fn wait_until(message: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT_TIMEOUT;
    loop {
        if condition() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out after {WAIT_TIMEOUT:?} waiting for: {message}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Retries `probe` (a blocking, deadline-guarded check such as
/// `has_server_side_provider`, per architecture section 4) up to
/// [`WAIT_TIMEOUT`], returning `true` as soon as it does. `UiaHasServerSideProvider`
/// blocks on the target's message pump and, under the heavy CPU contention
/// of a full-workspace `cargo test` run, can spuriously read as "no
/// provider" if mockapp's message loop is momentarily slow to answer rather
/// than genuinely absent; a single immediate check does not distinguish
/// those cases; retrying does.
pub fn eventually_true(mut probe: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + WAIT_TIMEOUT;
    loop {
        if probe() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
