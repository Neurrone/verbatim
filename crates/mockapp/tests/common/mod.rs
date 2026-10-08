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
use std::sync::OnceLock;
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

/// Puts this test process in a job of its own that ends every process in
/// it when this process ends, once, before its first child is started: every
/// child it starts from then on, `mockapp` included, is in that job from its
/// first instruction.
///
/// [`MockApp`]'s drop ends its process when a test fails by unwinding, but a
/// test process can end without unwinding: a panic in a callback the system
/// calls (a `WinEvent` hook's, a UIA handler's) cannot unwind and aborts the
/// process, and the harness ends its process with `TerminateProcess`
/// (`harness.rs`). A `mockapp` left running then keeps the standard error it
/// inherited open, and `cargo test`, which reads that to its end, waits for
/// it forever. With the job, the system ends every child when the job's one
/// handle closes, which it does as this process ends, however it ends.
///
/// # Panics
///
/// Panics if the job cannot be made, or this process cannot be put in it:
/// a child started then could outlive the test.
pub fn contain_children() {
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;
    /// The job's handle, held, never closed, for the life of this process.
    static JOB: OnceLock<usize> = OnceLock::new();
    JOB.get_or_init(|| {
        // SAFETY: no attributes and no name: a new unnamed job, whose
        // handle is not inheritable, so no child holds it open.
        let job = unsafe { CreateJobObjectW(None, PCWSTR::null()) }
            .unwrap_or_else(|error| panic!("the tests' job could not be made: {error}"));
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `limits` is the structure the information class names,
        // its size given.
        unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                    .expect("the structure's size fits"),
            )
        }
        .unwrap_or_else(|error| {
            panic!("the tests' job could not be set to end its processes: {error}")
        });
        // SAFETY: the pseudo-handle of this process; no preconditions.
        let this_process = unsafe { GetCurrentProcess() };
        // SAFETY: the job handle made above and this process's handle. A
        // process already in a job (cargo's) is put in this one nested
        // inside it.
        unsafe { AssignProcessToJobObject(job, this_process) }
            .unwrap_or_else(|error| panic!("the test process could not join its job: {error}"));
        job.0 as usize
    });
}

/// A running `mockapp` child process: killed on drop so a failing assertion
/// (which unwinds past the rest of the test) never leaves a window behind
/// to confuse the next test or a developer's desktop; and in this process's
/// job ([`contain_children`]), so it ends with this process however that
/// ends.
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

    /// Holds mockapp's next provider call, whichever client makes it, until
    /// [`release`](Self::release): returns once mockapp has taken the
    /// command, so every call from here on is the one held.
    pub fn hold(&mut self) {
        self.send("hold");
    }

    /// Waits for the call [`hold`](Self::hold) holds to begin, and returns
    /// the provider method it called, as mockapp names it: the evidence that
    /// a client's call is in progress.
    pub fn held(&mut self) -> String {
        let line = self.next_line("a call to be held", WAIT_TIMEOUT);
        line.strip_prefix("held ").map_or_else(
            || panic!("mockapp said {line:?}, not which call it held"),
            str::to_owned,
        )
    }

    /// Lets the call [`hold`](Self::hold) held go on, and returns once
    /// mockapp has taken the command.
    pub fn release(&mut self) {
        self.send("release");
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
    spawn_with(fixture, backend, title, &[])
}

/// [`spawn`], counting the client registrations for events on mockapp's
/// UIA root (`--count-registrations`), which [`advised`] reads. It changes
/// the provider calls UIA makes as clients register, so only the tests
/// that count registrations use it.
#[must_use]
pub fn spawn_counting_registrations(fixture: &str, title: &str) -> MockApp {
    spawn_with(fixture, "uia", title, &["--count-registrations"])
}

fn spawn_with(fixture: &str, backend: &str, title: &str, extra: &[&str]) -> MockApp {
    contain_children();
    let exe = env!("CARGO_BIN_EXE_mockapp");
    let mut child = Command::new(exe)
        .arg("--fixture")
        .arg(fixture_path(fixture))
        .arg("--backend")
        .arg(backend)
        .arg("--title")
        .arg(title)
        .args(extra)
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

    // The guard first, so a mockapp that never becomes ready is ended with
    // it.
    let app = MockApp {
        child,
        stdin,
        lines: rx,
        title: title.to_owned(),
    };
    match app.lines.recv_timeout(WAIT_TIMEOUT) {
        Ok(line) if line == "ready" => app,
        Ok(other) => panic!("mockapp's first stdout line was {other:?}, not \"ready\""),
        Err(mpsc::RecvTimeoutError::Timeout) => panic!(
            "mockapp did not print \"ready\" within {WAIT_TIMEOUT:?} (title {title:?}, fixture {fixture:?})"
        ),
        Err(mpsc::RecvTimeoutError::Disconnected) => panic!(
            "mockapp closed its output before printing \"ready\" (title {title:?}, fixture {fixture:?})"
        ),
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

/// How many client registrations for UIA event `event` are live on
/// mockapp's window, as UIA has reported them to its fragment root: added
/// less removed. Counted only by a mockapp started with
/// [`spawn_counting_registrations`].
#[must_use]
pub fn advised(hwnd: HWND, event: i32) -> i64 {
    // SAFETY: as in `reset_hits`; the event id travels as a plain integer.
    let count = unsafe {
        windows::Win32::UI::WindowsAndMessaging::SendMessageW(
            hwnd,
            hits::WM_ADVISED_READ,
            Some(windows::Win32::Foundation::WPARAM(
                usize::try_from(event).unwrap_or_default(),
            )),
            None,
        )
    }
    .0;
    i64::try_from(count).unwrap_or(i64::MAX)
}

/// Sends `line` to mockapp and waits until it has taken effect, then zeroes
/// the hit counters, so what is measured next starts from nothing.
pub fn apply(app: &mut MockApp, hwnd: HWND, line: &str) {
    app.send(line);
    reset_hits(hwnd);
}
