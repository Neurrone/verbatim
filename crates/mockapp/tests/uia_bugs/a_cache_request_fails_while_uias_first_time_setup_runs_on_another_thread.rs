//! `docs/uia-bugs.md`, "A cache request fails while UIA's first-time setup
//! runs on another thread": several threads of a fresh process, as an
//! outpost starts its UIA threads together, each create a client and build
//! a cache request at once. They all succeed only because `verbatim_uia`
//! finishes UIA's first-time setup once before any thread uses UIA.
//!
//! The setup runs once in a process, so each race is run in a fresh
//! process of its own, this test binary started again ([`racer`]), and
//! several are run: without the workaround about one race in 10 to 30
//! found no failure, so several together leave the fault no way to pass
//! unseen.

use std::io::Read as _;
use std::process::{Command, Stdio};
use std::sync::mpsc;

use verbatim_uia::Shape as _;
use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};

use crate::common;

/// The environment variable that makes this binary a racing process.
pub const RACER: &str = "VERBATIM_TEST_SETUP_RACER";

/// The threads that race in each process.
const THREADS: usize = 6;

/// The racing processes.
const PROCESSES: usize = 4;

/// Each of [`THREADS`] threads creates a client and builds a cache request
/// at once; how each ended.
fn race() -> Vec<Result<(), String>> {
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(THREADS));
    let threads: Vec<_> = (0..THREADS)
        .map(|_| {
            let barrier = std::sync::Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                let uia = verbatim_uia::Uia::new().map_err(|error| format!("client: {error}"))?;
                uia.request::<verbatim_uia::Node>(verbatim_uia::Node::all_on())
                    .map(|_| ())
                    .map_err(|error| format!("cache request: {error}"))
            })
        })
        .collect();
    threads
        .into_iter()
        .map(|thread| thread.join().expect("the thread finishes"))
        .collect()
}

/// A racing process, run as this binary with [`RACER`] set: runs the race
/// and writes how each thread ended, then ends without running any DLL's
/// detach code, as the test runner does (`common/harness.rs`).
pub fn racer() -> ! {
    println!("{:?}", race());
    // SAFETY: the pseudo-handle of this process; no preconditions.
    let this_process = unsafe { GetCurrentProcess() };
    // SAFETY: ends this process; nothing runs after it.
    let _ = unsafe { TerminateProcess(this_process, 0) };
    std::process::abort();
}

/// In each of [`PROCESSES`] fresh processes, racing at once, every one of
/// [`THREADS`] threads that create a client and build a cache request at
/// once succeeds.
pub fn clients_created_at_once_in_a_fresh_process_all_build_cache_requests() {
    common::contain_children();
    let (done, raced) = mpsc::channel();
    for _ in 0..PROCESSES {
        let mut child = Command::new(std::env::current_exe().expect("this test binary"))
            .env(RACER, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("a racing process started");
        let mut stdout = child.stdout.take().expect("the racing process's output");
        let done = done.clone();
        std::thread::spawn(move || {
            let mut written = String::new();
            let read = stdout.read_to_string(&mut written);
            let status = child.wait();
            let _ = done.send((read.map(|_| written), status.map(|status| status.success())));
        });
    }
    let all_ok = format!("{:?}\n", vec![Ok::<(), String>(()); THREADS]);
    for _ in 0..PROCESSES {
        let (written, succeeded) = raced
            .recv_timeout(common::WAIT_TIMEOUT)
            .expect("a racing process ends");
        assert_eq!(
            (
                written.expect("its output read"),
                succeeded.expect("its exit read")
            ),
            (all_ok.clone(), true)
        );
    }
}
